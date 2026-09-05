"""Tests for AtlasAgent multi-turn chat functionality."""

import pytest
from pathlib import Path
from unittest.mock import MagicMock, patch

from atlas.llm import StubClient


def _make_agent(tmp_path, stub_client=None):
    """Create an AtlasAgent with mocked dependencies for chat testing."""
    with patch("atlas.agent.RepositorySession"), \
         patch("atlas.agent.SessionContext") as MockContextManager:
        mock_cm = MockContextManager.return_value
        mock_cm.assemble_context.return_value = ""

        from atlas.agent import AtlasAgent

        agent = AtlasAgent(project_root=tmp_path)
        if stub_client:
            agent.llm = stub_client
        return agent


class TestSingleTurnWithoutTools:
    """Chat with no tool calls should complete in one round."""

    def test_single_turn_plain_response(self, tmp_path):
        agent = _make_agent(tmp_path)
        # Default StubClient returns plain text for non-architecture queries
        agent.chat("Hello, how are you?")

        # Should have 2 messages: user + assistant
        assert len(agent.conversation_history) == 2
        assert agent.conversation_history[0]["role"] == "user"
        assert agent.conversation_history[1]["role"] == "assistant"


class TestAgenticLoop:
    """Chat with tool calls should loop: tool call -> result -> final answer."""

    def test_tool_call_then_final_answer(self, tmp_path):
        """StubClient returns a read_file action for 'architecture', then a final answer on tool result."""
        stub = StubClient()
        agent = _make_agent(tmp_path, stub_client=stub)

        # Create a file the tool can read
        test_file = agent.tools.project_root / "roadmap.md"
        test_file.write_text("# Roadmap\nPhase 1: Build core")

        agent.chat("Tell me about the architecture")

        # StubClient should have been called twice:
        # 1. Initial query -> returns read_file action
        # 2. Tool result follow-up -> returns final answer
        assert stub.call_count == 2

        # History should contain: user, assistant (tool call), user (tool result), assistant (final)
        assert len(agent.conversation_history) == 4
        assert agent.conversation_history[0]["role"] == "user"
        assert agent.conversation_history[1]["role"] == "assistant"
        assert agent.conversation_history[2]["role"] == "user"
        assert "## Tool Results" in agent.conversation_history[2]["content"]
        assert agent.conversation_history[3]["role"] == "assistant"


class TestMaxRoundsTermination:
    """Chat should stop after max_tool_rounds if tools keep being requested."""

    def test_max_rounds_stops_loop(self, tmp_path):
        class AlwaysToolClient(StubClient):
            def chat(self, messages):
                self.call_count += 1
                return "<action>read_file('test.py')</action>"

        stub = AlwaysToolClient()
        agent = _make_agent(tmp_path, stub_client=stub)

        # Create a file the tool can read
        test_file = agent.tools.project_root / "test.py"
        test_file.write_text("x = 1")

        agent.chat("loop forever", max_tool_rounds=3)

        # Should stop after 3 rounds
        assert stub.call_count == 3


class TestHistoryTrimming:
    """_trim_history should drop oldest messages when budget is exceeded."""

    def test_trimming_drops_old_messages(self, tmp_path):
        agent = _make_agent(tmp_path)
        agent.max_history_tokens = 100  # Very small budget

        # Add many messages to history
        for i in range(20):
            agent.conversation_history.append({
                "role": "user",
                "content": f"Message {i} with some content to take up tokens " * 10
            })

        trimmed = agent._trim_history()

        # Should have fewer messages than the full history
        assert len(trimmed) < len(agent.conversation_history)
        # Last message should be preserved (most recent)
        assert trimmed[-1] == agent.conversation_history[-1]


class TestMultiTurnContextPreservation:
    """Multiple chat() calls should preserve conversation history."""

    def test_history_persists_across_turns(self, tmp_path):
        agent = _make_agent(tmp_path)

        agent.chat("First question")
        first_turn_len = len(agent.conversation_history)

        agent.chat("Follow-up question")
        second_turn_len = len(agent.conversation_history)

        # Second turn should have more history than first
        assert second_turn_len > first_turn_len


class TestResetConversation:
    """reset_conversation() should clear all history."""

    def test_reset_clears_history(self, tmp_path):
        agent = _make_agent(tmp_path)

        agent.chat("Some question")
        assert len(agent.conversation_history) > 0

        agent.reset_conversation()
        assert len(agent.conversation_history) == 0


class TestFormatToolResults:
    """Test _format_tool_results with various result types."""

    def test_successful_read(self, tmp_path):
        agent = _make_agent(tmp_path)
        results = [{
            "tool": "read_file",
            "args": ["main.py"],
            "result": {"success": True, "content": "x = 1", "lines": 1}
        }]
        formatted = agent._format_tool_results(results)

        assert "read_file" in formatted
        assert "SUCCESS" in formatted
        assert "x = 1" in formatted

    def test_failed_tool(self, tmp_path):
        agent = _make_agent(tmp_path)
        results = [{
            "tool": "read_file",
            "args": ["missing.py"],
            "result": {"success": False, "error": "File not found"}
        }]
        formatted = agent._format_tool_results(results)

        assert "FAILED" in formatted
        assert "File not found" in formatted

    def test_truncation_of_large_content(self, tmp_path):
        agent = _make_agent(tmp_path)
        large_content = "x" * 10000
        results = [{
            "tool": "read_file",
            "args": ["big.py"],
            "result": {"success": True, "content": large_content, "lines": 1}
        }]
        formatted = agent._format_tool_results(results)

        assert "[... truncated ...]" in formatted
        assert len(formatted) < len(large_content)

    def test_directory_listing(self, tmp_path):
        agent = _make_agent(tmp_path)
        results = [{
            "tool": "list_directory",
            "args": ["."],
            "result": {"success": True, "entries": ["a.py", "b.py", "c.py"]}
        }]
        formatted = agent._format_tool_results(results)

        assert "a.py" in formatted
        assert "b.py" in formatted

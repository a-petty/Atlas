import asyncio
from pathlib import Path
import os
import pytest
import logging
from io import StringIO
import tempfile
import shutil

from atlas.agent import AtlasAgent

@pytest.mark.asyncio
async def test_graceful_handling_of_syntax_error():
    """
    Tests that the agent can gracefully handle a valid file that is updated
    to contain a syntax error. The agent should log the ParseError but not crash.
    """
    # 1. Create a temporary directory for an isolated test environment
    with tempfile.TemporaryDirectory() as temp_dir_str:
        temp_dir = Path(temp_dir_str)
        print(f"--- Starting Auto-Recovery Test in temp dir: {temp_dir} ---")

        # 2. Create a dummy file with VALID content first
        dummy_file = temp_dir / "dummy_file_to_break.py"
        dummy_file.write_text("def my_valid_func():\n  return 'hello world'\n")
        print(f"Created valid dummy file: {dummy_file.name}")

        # 3. Instantiate the agent with the temporary directory as the project root
        agent = AtlasAgent(project_root=temp_dir)
        
        print("\nInitializing agent...")
        agent.initialize()
        
        # 4. Now, overwrite the file with content containing a syntax error
        print(f"\nIntroducing syntax error into {dummy_file.name}...")
        dummy_file.write_text("def my_func():\n  print('hello world')\n\nthis is a syntax error")

        # 5. Set up a logger to capture the error messages
        log = logging.getLogger("atlas")
        stream = StringIO()
        test_handler = logging.StreamHandler(stream)
        log.addHandler(test_handler)

        # 6. Call the handler to simulate the file modification event
        absolute_dummy_path = dummy_file.resolve()
        print(f"\nSimulating modification event for: {absolute_dummy_path}")
        agent._handle_file_modified(absolute_dummy_path)

        # Remove the handler so we don't affect other tests
        log.removeHandler(test_handler)

        output = stream.getvalue()
        print("--- Captured Log Output ---")
        print(output)
        print("-------------------------")

        # 7. Assert that the correct error message was logged
        assert "Syntax error in" in output, "The log should indicate a syntax error."
        assert "excluded from current graph" in output
        assert agent.session.request("status")["coverage"]["indexed"] == 0
        dummy_file.write_text("def repaired():\n    return 1\n")
        assert agent.session.request("status")["coverage"]["indexed"] == 1
        agent.stop()

    print("\n--- Test Finished Successfully ---")
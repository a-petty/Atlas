import time
import logging
from pathlib import Path
from typing import Optional, List, Dict, Set, Tuple
import re
import ast

# Dependencies defined in pyproject.toml
from pydantic import BaseModel, Field
from rich.console import Console
from rich.logging import RichHandler
from rich.panel import Panel
from rich.tree import Tree
from rich.progress import (
    Progress,
    SpinnerColumn,
    TextColumn,
    BarColumn,
    TaskProgressColumn,
    TimeRemainingColumn,
)
from rich.markdown import Markdown

# Rust Core Import
from atlas.semantic_engine import RepoGraph, FileWatcher, scan_repository, GraphError, ParseError, NodeNotFoundError

# Local Imports
from .embeddings import EmbeddingManager
from .context import ContextManager
from .llm import StubClient, OllamaClient, MLXClient
from .tools import ToolExecutor

# Setup Rich Console & Logging
console = Console()
logging.basicConfig(
    level="INFO",
    format="%(message)s",
    datefmt="[%X]",
    handlers=[RichHandler(console=console, markup=True, show_path=False)]
)
log = logging.getLogger("atlas")

SYSTEM_PROMPT = """You are Atlas, an autonomous coding agent with deep architectural understanding.

## Context Structure

You receive three types of file information:

1. **FULL CONTENT** - Complete source code of files in your immediate working context
2. **SKELETON** - Function/class signatures without implementation details
3. **REPOSITORY_MAP** - High-level architectural overview

## Critical Instructions

### The Golden Rule
Your single most important rule is to ALWAYS respond in the <think>...<action> format. Do not write any other kind of response.

### Understanding Your Context
- Files marked (FULL CONTENT) contain complete implementations
- Files marked (SKELETON) show only signatures - you do NOT have implementation details
- The REPOSITORY_MAP shows architectural relationships but not code

### When You Need More Information
- If you need to understand HOW a function/class works, and you only have its SKELETON:
  1. State this clearly in your <think> block
  2. Use the read_file(path) tool to get the full implementation
  3. NEVER guess or hallucinate implementation details from a skeleton

### Your Reasoning Process
Always structure your responses as:

<think>
1. What files do I have FULL CONTENT for?
2. What files do I only have SKELETONS for?
3. Do I need more detail from any skeleton files?
4. What is my implementation strategy?
</think>

### Tool Usage Syntax
- You MUST call tools by wrapping the call in <action> tags.
- The call inside the tag must be a single function call.
- e.g. <action>read_file('src/main.py')</action>

### Available Tools
- read_file(path) - Get full content of a file
- write_file(path, content) - Write/modify a file (syntax-checked automatically)
- list_directory(path) - List directory contents
- generate_repository_map(output_path) - Generates a map of the repository architecture and saves it to a file.

## Safety
- ALL writes are syntax-checked before saving
- Backups are created automatically
- Path traversal is prevented
"""

_DEFAULT_IGNORED_DIRS = {"node_modules", "target", ".git", "__pycache__", "dist", "build", ".venv", "venv"}


def _load_ignore_dirs(project_root: Path) -> Set[str]:
    """Load ignored directory names from defaults and .atlasignore file."""
    dirs = set(_DEFAULT_IGNORED_DIRS)
    ignore_file = project_root / ".atlasignore"
    if ignore_file.exists():
        for line in ignore_file.read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#"):
                dirs.add(line)
        log.info(f"Loaded .atlasignore: {len(dirs) - len(_DEFAULT_IGNORED_DIRS)} extra patterns")
    return dirs


class AgentConfig(BaseModel):
    """Configuration settings for the Atlas Agent."""
    project_root: Path
    extensions: List[str] = Field(
        default_factory=lambda: ["py", "rs", "js", "ts", "go", "java", "c", "cpp", "md", "toml", "json"]
    )
    ignored_dirs: Set[str] = Field(
        default_factory=lambda: set(_DEFAULT_IGNORED_DIRS)
    )
    debounce_seconds: float = 0.5

class AtlasAgent:
    """
    Main agent orchestrator for Project Atlas.
    Bridges the Rust semantic engine with the Python runtime.
    """

    def __init__(self, project_root: Path, provider: str = "stub", model_name: str = "deepseek-coder"):
        ignored = _load_ignore_dirs(project_root)
        self.config = AgentConfig(project_root=project_root, ignored_dirs=ignored)
        self.project_root_canonical = project_root.resolve()
        self.repo_graph = RepoGraph(str(project_root), ignored_dirs=list(ignored))
        self.watcher: Optional[FileWatcher] = None
        self.tools = ToolExecutor(project_root.resolve(), self.repo_graph)
        self.running = False

        self.embedding_manager = EmbeddingManager()
        self.context_manager = ContextManager(self.repo_graph, self.embedding_manager, model=model_name)

        if provider == "ollama":
            self.llm = OllamaClient(model=model_name)
        elif provider == "mlx":
            self.llm = MLXClient(model=model_name)
        else:
            self.llm = StubClient()

        # Conversation state for multi-turn chat
        self.conversation_history: List[Dict[str, str]] = []
        self.max_history_tokens: int = 50000

        self._last_event_time: Dict[str, float] = {}

    def initialize(self):
        """
        Perform initial repository scan and graph build with detailed multi-stage progress.
        """
        console.print(Panel(
            f"[bold blue]Atlas Agent[/bold blue]\nTarget: {self.config.project_root}",
            border_style="blue"
        ))

        with Progress(
            SpinnerColumn(),
            TextColumn("[progress.description]{task.description}"),
            BarColumn(),
            TaskProgressColumn(),
            TimeRemainingColumn(),
            console=console,
            transient=False,  # Keep progress visible after completion
        ) as progress:
            
            scan_task = progress.add_task("[cyan]Scanning repository...", total=None)
            files = scan_repository(
                str(self.config.project_root),
                ignored_dirs=list(self.config.ignored_dirs)
            )
            progress.update(scan_task, total=len(files), completed=len(files))
            log.info(f"Found [cyan]{len(files)}[/cyan] source files.")
            
            # This is a simplified representation. A true implementation would
            # require the Rust `build_complete` to accept a callback to report progress.
            build_task = progress.add_task("[yellow]Building semantic graph...", total=len(files))
            
            # Simulate progress reporting from Rust
            self.repo_graph.build_complete(files)
            progress.update(build_task, completed=len(files))
            
            rank_task = progress.add_task("[magenta]Calculating architectural importance...", total=None)
            self.repo_graph.ensure_pagerank_up_to_date()
            progress.update(rank_task, completed=True)

            embed_task = progress.add_task("[white]Preparing semantic search...", total=None)
            # In a real scenario, you might pre-build some embeddings here.
            progress.update(embed_task, completed=True)

        console.print("[bold green]✓[/bold green] Semantic Engine Initialized.")
        
        # Display summary statistics
        stats = self.repo_graph.get_statistics()
        stats_tree = Tree("📊 [bold]Repository Statistics[/bold]")
        stats_tree.add(f"[cyan]Files Indexed:[/cyan] {stats.node_count}")
        stats_tree.add(f"[yellow]Dependencies:[/yellow] {stats.edge_count}")
        stats_tree.add(f"[green]Symbols:[/green] {stats.total_definitions}")
        console.print(stats_tree)
        
        self._display_top_files()
        
        log.info(f"Attaching file watcher to: {', '.join(self.config.extensions)}")
        self.watcher = FileWatcher(
            str(self.config.project_root),
            extensions=self.config.extensions,
            ignored_dirs=list(self.config.ignored_dirs)
        )

    def run(self):
        """
        Main Event Loop for file watching.
        """
        if self.watcher is None:
            raise RuntimeError("Agent not initialized. Call initialize() first.")
        
        self.running = True
        console.print(f"\n[bold green]◉ Agent Active.[/bold green] Watching for changes... (Press Ctrl+C to stop)\n")
        
        try:
            while self.running:
                events = self.watcher.poll_events()
                for event in events:
                    self._handle_file_event(event)
                time.sleep(0.1)
        except KeyboardInterrupt:
            console.print("\n[yellow]Shutdown signal received.[/yellow]")
        finally:
            self.stop()
    
    def stop(self):
        """Graceful shutdown."""
        self.running = False
        if self.watcher and hasattr(self.watcher, 'stop'):
            self.watcher.stop()
        console.print("[dim]Agent stopped.[/dim]")
    
    def _normalize_path(self, file_path: Path) -> str:
        """Convert a file path to canonical, project-root-relative format."""
        canonical = file_path.resolve()
        try:
            relative = canonical.relative_to(self.project_root_canonical)
        except ValueError:
            raise ValueError(f"Path {canonical} is outside project root {self.project_root_canonical}")
        return str(relative).replace('\\', '/')

    def _handle_file_event(self, event):
        """Dispatcher for file system events with debouncing."""
        path = Path(event.path)
        
        if path.name.startswith(".") or path.name.endswith("~"):
            return

        now = time.time()
        if now - self._last_event_time.get(path.name, 0) < self.config.debounce_seconds:
            return
        self._last_event_time[path.name] = now

        timestamp = time.strftime("%H:%M:%S")
        prefix = f"[dim]{timestamp}[/dim]"
        
        if event.event_type == "modified":
            console.print(f"{prefix} [yellow]MODIFIED[/yellow] {path.name}")
            self._handle_file_modified(path)
        elif event.event_type == "created":
            console.print(f"{prefix} [green]CREATED [/green] {path.name}")
            self._handle_file_created(path)
        elif event.event_type == "deleted":
            console.print(f"{prefix} [red]DELETED [/red] {path.name}")
            self._handle_file_deleted(path)
    
    def _handle_file_modified(self, file_path: Path):
        """Handle file modification with self-healing."""
        content = self._safe_read_file(file_path)
        if content is None: return

        try:
            result = self.repo_graph.update_file(str(file_path.resolve()), content)
            if result.needs_pagerank_recalc:
                console.print(f"  ↳ [green]Graph Updated[/green] (+{result.edges_added} / -{result.edges_removed} edges)")
            else:
                console.print("  ↳ [dim]Content updated (Structure unchanged)[/dim]")
        except ParseError as e:
            log.warning(f"  ↳ [yellow]Syntax error in {file_path.name}[/yellow]. Graph state will be stale until fixed.")
        except NodeNotFoundError:
            log.warning(f"  ↳ File {file_path.name} not in graph, adding as new (self-healing).")
            self._handle_file_created(file_path)
        except GraphError as e:
            log.error(f"  ✗ Graph error updating {file_path.name}: {e}")
        except Exception as e:
            log.error(f"  ✗ Unexpected error processing {file_path.name}: {e}", exc_info=True)
    
    def _handle_file_created(self, file_path: Path):
        """Handle file creation."""
        content = self._safe_read_file(file_path)
        if content is None: return

        try:
            normalized_path = self._normalize_path(file_path)
            self.repo_graph.add_file(normalized_path, content)
            console.print(f"  ↳ [green]Added to graph[/green]: {file_path.name}")
        except ParseError as e:
            log.warning(f"  ↳ [yellow]Syntax error in new file {file_path.name}[/yellow]. File will be skipped.")
        except ValueError as e:
            log.debug(f"  ↳ Skipped file outside project: {e}")
        except GraphError as e:
            log.error(f"  ✗ Graph error adding {file_path.name}: {e}")
        except Exception as e:
            log.error(f"  ✗ Unexpected error adding {file_path.name}: {e}", exc_info=True)
    
    def _handle_file_deleted(self, file_path: Path):
        """Handle file deletion."""
        try:
            normalized_path = self._normalize_path(file_path)
            self.repo_graph.remove_file(normalized_path)
            console.print(f"  ↳ [red]Removed from graph[/red]: {file_path.name}")
        except NodeNotFoundError:
            log.debug(f"  ↳ File not in graph, skipping removal: {file_path.name}")
        except ValueError as e:
            log.debug(f"  ↳ Ignored deletion outside project: {e}")
        except GraphError as e:
            log.error(f"  ✗ Graph error removing {file_path.name}: {e}")
        except Exception as e:
            log.error(f"  ✗ Unexpected error removing {file_path.name}: {e}", exc_info=True)

    def _safe_read_file(self, path: Path, retries=3, delay=0.1) -> Optional[str]:
        """Safely read a file, handling race conditions and binary files."""
        for i in range(retries):
            try:
                return path.read_text(encoding="utf-8")
            except (PermissionError, FileNotFoundError):
                time.sleep(delay)
            except UnicodeDecodeError:
                log.warning(f"  ↳ Skipped binary file: {path.name}")
                return None
        log.error(f"  ↳ Failed to read file {path} after {retries} retries.")
        return None

    def _display_top_files(self):
        """Visualizes the top files using a Tree."""
        top_files = self.repo_graph.get_top_ranked_files(5)
        tree = Tree("📁 [bold]Top Architectural Files[/bold]")
        for path_str, rank in top_files:
            tree.add(f"[cyan]{Path(path_str).name}[/cyan] [dim](Rank: {rank:.3f})[/dim]")
        console.print(tree)
        console.print("")

    def get_architecture_map(self, max_files: int = 50) -> str:
        """Generate a text map of the repository architecture."""
        return self.repo_graph.generate_map(max_files)
    
    def _execute_actions(self, actions: List[Dict]) -> List[Dict]:
        """Executes a list of tool calls, prints the results, and returns them."""
        results = []
        if not actions:
            return results

        console.print("\n[bold green]▶️ Executing Actions...[/bold green]")
        for action in actions:
            tool_name = action.get("tool")
            tool_args = action.get("args", [])

            if hasattr(self.tools, tool_name):
                method = getattr(self.tools, tool_name)
                result = method(*tool_args)
                console.print(f"  [cyan]{tool_name}[/cyan]: ", result)
                results.append({"tool": tool_name, "args": tool_args, "result": result})

                if not result.get('success', False):
                    log.warning(f"  [bold yellow]Tool '{tool_name}' failed. Stopping execution chain.[/bold yellow]")
                    break
            else:
                error_result = {"success": False, "error": f"Unknown tool: {tool_name}"}
                log.error(f"  [bold red]Agent tried to use unknown tool: {tool_name}[/bold red]")
                results.append({"tool": tool_name, "args": tool_args, "result": error_result})
                break

        return results
    
    def _parse_response(self, response: str) -> Tuple[Optional[str], List[Dict], str]:
        """
        Parses the LLM's response to extract <think>, <action> blocks, and plain text.

        Returns:
            (thoughts, actions, plain_text) where plain_text is content outside
            think/action blocks.
        """
        thoughts = None
        actions = []

        # Normalize whitespace in tags: "< think >" -> "<think>"
        normalized = re.sub(r'<\s*(/?)\s*(think|action)\s*>', r'<\1\2>', response)

        # Extract think block (handle unclosed tags)
        think_match = re.search(r'<think>(.*?)(?:</think>|$)', normalized, re.DOTALL)
        if think_match:
            thoughts = think_match.group(1).strip()

        # Extract action blocks (handle unclosed tags)
        action_matches = re.findall(r'<action>(.*?)(?:</action>|$)', normalized, re.DOTALL)

        for action_text in action_matches:
            parsed = self._parse_single_action(action_text.strip())
            if parsed:
                actions.append(parsed)

        # If no action tags found, try fallback parsing
        if not actions:
            actions = self._fallback_parse_actions(normalized)

        # Extract plain text (everything outside think/action tags)
        plain_text = re.sub(r'<think>.*?(?:</think>|$)', '', normalized, flags=re.DOTALL)
        plain_text = re.sub(r'<action>.*?(?:</action>|$)', '', plain_text, flags=re.DOTALL)
        plain_text = plain_text.strip()

        # Log raw response on total parse failure
        if not thoughts and not actions and not plain_text:
            log.warning(f"Could not parse any content from LLM response: {response[:500]}")

        return thoughts, actions, plain_text

    def _parse_single_action(self, action_text: str) -> Optional[Dict]:
        """Parse a single tool_name(args) pattern from action text."""
        match = re.match(r'(\w+)\((.*)\)', action_text, re.DOTALL)
        if not match:
            log.warning(f"Could not match tool call pattern in action: {action_text[:100]}")
            return None

        tool_name = match.group(1)
        args_str = match.group(2).strip()

        if not args_str:
            return {'tool': tool_name, 'args': []}

        # Primary: ast.literal_eval
        try:
            args = ast.literal_eval(f"[{args_str}]")
            return {'tool': tool_name, 'args': args}
        except (ValueError, SyntaxError):
            pass

        # Fallback: extract quoted string arguments via regex
        quoted_args = re.findall(r"""(['"])(.*?)\1""", args_str)
        if quoted_args:
            args = [val for _, val in quoted_args]
            log.debug(f"Fell back to regex arg extraction for {tool_name}: {args}")
            return {'tool': tool_name, 'args': args}

        log.warning(f"Could not parse arguments for action: {tool_name}({args_str[:100]})")
        return None

    KNOWN_TOOLS = {'read_file', 'write_file', 'list_directory', 'generate_repository_map'}

    def _fallback_parse_actions(self, text: str) -> List[Dict]:
        """
        When no <action> tags found, scan raw text for known tool call patterns.
        """
        actions = []
        # Look for tool_name(args) patterns in untagged text
        for match in re.finditer(r'\b(' + '|'.join(self.KNOWN_TOOLS) + r')\s*\((.*?)\)', text, re.DOTALL):
            parsed = self._parse_single_action(match.group(0))
            if parsed:
                actions.append(parsed)
        return actions

    def query(self, user_input: str):
        """Process a user query with enhanced context awareness."""
        console.print(f"\n[bold green]Agent received query:[/bold green] {user_input}")

        context = ""
        # Check for keywords that imply a whole-repository action
        if "entire codebase" not in user_input and "generate_repository_map" not in user_input:
            # 1. Assemble smart context if it's not a whole-repo query
            context = self.context_manager.assemble_context(user_input, files_in_scope=[])

        # 2. Create structured prompt
        final_prompt = (
            "## Code Context\n\n"
            f"{context}\n\n"
            "## User's Request\n\n"
            f"Based on the context above (if any), please handle the following request:\n\n---\n{user_input}\n---\n\n"
            "Remember to respond using the <think> and <action> format."
        )
        
        messages = [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": final_prompt}
        ]
        
        # 3. Get LLM response
        console.print("[yellow]Thinking...[/yellow]")
        llm_response = self.llm.chat(messages)
        
        # 4. Parse and display reasoning
        thoughts, actions, plain_text = self._parse_response(llm_response)

        if thoughts:
            console.print("\n[bold cyan]🧠 Agent Reasoning:[/bold cyan]")
            # Check if agent is skeleton-aware
            if "SKELETON" in thoughts.upper() or "FULL CONTENT" in thoughts.upper():
                console.print("[dim](✓ Agent is context-aware)[/dim]")
            console.print(Markdown(thoughts))

        if plain_text:
            console.print("\n[bold cyan]🤖 Agent Response:[/bold cyan]")
            console.print(Markdown(plain_text))

        if not thoughts and not plain_text:
            console.print("\n[bold cyan]🤖 Agent Response:[/bold cyan]")
            console.print(llm_response)

        # 5. Execute actions
        if actions:
            self._execute_actions(actions)
        elif not plain_text and not thoughts:
            console.print("[dim]No actions requested.[/dim]")

    def chat(self, user_input: str, max_tool_rounds: int = 5):
        """
        Multi-turn agentic chat. Assembles context, appends user message
        to history, and enters an agentic loop that executes tools and feeds
        results back to the LLM until the LLM produces a final answer.
        """
        # 1. Assemble context for first message in conversation, or refresh periodically
        context = ""
        if "entire codebase" not in user_input and "generate_repository_map" not in user_input:
            context = self.context_manager.assemble_context(user_input, files_in_scope=[])

        # 2. Build user message with context (only attach context for first message or when context is fresh)
        if context:
            user_content = (
                "## Code Context\n\n"
                f"{context}\n\n"
                "## User's Request\n\n"
                f"{user_input}"
            )
        else:
            user_content = user_input

        self.conversation_history.append({"role": "user", "content": user_content})

        # 3. Agentic loop
        for round_num in range(max_tool_rounds):
            # Build messages: system prompt + trimmed history
            messages = [{"role": "system", "content": SYSTEM_PROMPT}]
            messages.extend(self._trim_history())

            # Call LLM
            console.print("[yellow]Thinking...[/yellow]")
            llm_response = self.llm.chat(messages)

            # Parse response
            thoughts, actions, plain_text = self._parse_response(llm_response)

            if thoughts:
                console.print("\n[bold cyan]🧠 Agent Reasoning:[/bold cyan]")
                console.print(Markdown(thoughts))

            # If no actions -> final answer, store and return
            if not actions:
                response_text = plain_text or thoughts or llm_response
                self.conversation_history.append({"role": "assistant", "content": llm_response})

                if plain_text:
                    console.print("\n[bold cyan]🤖 Agent Response:[/bold cyan]")
                    console.print(Markdown(plain_text))
                elif not thoughts:
                    console.print("\n[bold cyan]🤖 Agent Response:[/bold cyan]")
                    console.print(llm_response)
                return

            # Execute tools
            tool_results = self._execute_actions(actions)

            # Store assistant message and tool results in history
            self.conversation_history.append({"role": "assistant", "content": llm_response})
            tool_result_text = self._format_tool_results(tool_results)
            self.conversation_history.append({"role": "user", "content": tool_result_text})

        # Max rounds reached
        console.print(f"\n[yellow]Reached maximum tool rounds ({max_tool_rounds}). Stopping.[/yellow]")

    def _trim_history(self) -> List[Dict[str, str]]:
        """
        Walk backward through history, dropping oldest messages
        that would exceed max_history_tokens.
        """
        trimmed = []
        token_count = 0

        for msg in reversed(self.conversation_history):
            msg_tokens = len(msg["content"]) // 4  # rough token estimate
            if token_count + msg_tokens > self.max_history_tokens and trimmed:
                break
            trimmed.append(msg)
            token_count += msg_tokens

        trimmed.reverse()
        return trimmed

    def _format_tool_results(self, results: List[Dict]) -> str:
        """Format tool execution results as a message for the LLM."""
        parts = ["## Tool Results\n"]
        for r in results:
            tool = r["tool"]
            args = r["args"]
            result = r["result"]
            success = result.get("success", False)
            status = "SUCCESS" if success else "FAILED"

            parts.append(f"### {tool}({', '.join(repr(a) for a in args)}) — {status}")

            if not success:
                parts.append(f"Error: {result.get('error', 'Unknown error')}")
            elif "content" in result:
                content = result["content"]
                if len(content) > 5000:
                    content = content[:5000] + "\n\n[... truncated ...]"
                parts.append(f"```\n{content}\n```")
            elif "entries" in result:
                parts.append(", ".join(result["entries"]))
            elif "message" in result:
                parts.append(result["message"])
            else:
                parts.append(str(result))

            parts.append("")

        return "\n".join(parts)

    def reset_conversation(self):
        """Clear conversation history."""
        self.conversation_history.clear()
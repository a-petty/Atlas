 How Atlas Works: A Complete Walkthrough
                                                                                                                                                    
  Layer 1: The Rust Core — Building a Semantic Model of Code                                                                                      
                                                                                                                                                  
  At the foundation, Atlas is a graph compiler for source code. It takes a repository of files and produces two layered graph structures that encode what the code means structurally, not just what it says textually.

  The File-Level Graph (graph.rs)

  The primary data structure is a DiGraph<FileNode, EdgeKind> from petgraph. Every source file becomes a FileNode containing:
  - The file's path
  - A list of symbol definitions (functions, classes, variables it exports)
  - A list of symbol usages (symbols from other files it references)
  - A PageRank score (initially 0, computed after graph construction)
  - Content hashes for imports, definitions, usages, and full content (used for incremental change detection)

  Edges between file nodes are one of two kinds:
  - Import (strength 1.0) — File A has an import statement pointing to File B
  - SymbolUsage (strength 2.0) — File A references a symbol defined in File B (stronger signal than a bare import because it means the code actually uses the dependency, not just declares it)

  How the graph gets built — build() method, 3 stages:

  1. Parallel parse — Rayon spawns threads, each with its own ParserPool (one Tree-sitter parser per language). Every file is parsed concurrently.
  The output is a FileParseResult containing the Tree-sitter tree, extracted symbols (via SymbolHarvester), and resolved imports (via
  ImportResolver).
  2. Serial node insertion — All FileNodes are created and added to the petgraph. This must be serial because petgraph's add_node returns a NodeIndex that other data structures need to reference. A path_to_idx HashMap provides O(1) path→node lookup.
  3. Edge creation — For each file:
    - Import edges: The import resolver already mapped import statements to file paths. For each resolved import, add an Import edge from importer → imported file.
    - Symbol usage edges: For each symbol the file uses, look up symbol_index.definitions[symbol_name] to find which file defines it. If found (and it's not a self-reference), add a SymbolUsage edge.

  After edges exist, PageRank runs. It's a standard iterative algorithm with one twist: edge weights matter. SymbolUsage edges carry 2x the weight of Import edges when distributing rank, so files that are actually called get more rank than files that are merely imported. The result: core utility files that many other files depend on float to the top.

  How Parsing Works (parser.rs)

  SymbolHarvester uses language-specific Tree-sitter queries (S-expressions) to extract symbols. For Python, the query pattern matches
  function_definition, class_definition, decorated_definition, and module-level assignments. Each match produces a Symbol struct with name, kind (Function/Class/Method/Variable), byte offsets, and whether it's a definition or usage.

  The harvester distinguishes definitions from usages by node type: a function_definition is a definition, while an identifier reference in expression context is a usage. These get separated into the FileNode.definitions and FileNode.usages vectors.

  Skeleton generation is a separate operation. create_skeleton() walks the Tree-sitter tree, identifies "keeper" ranges (imports, function/class signatures, docstrings, decorators), and replaces everything else with .... This produces a compressed representation — typically 70-90% smaller — that preserves the full API surface of a file.

  How Import Resolution Works (import_resolver.rs)

  The ImportResolver trait has language-specific implementations. PythonImportResolver works like this at startup:

  1. Walk the project directory for all .py/.pyi files
  2. Build a module_index: HashMap<String, PathBuf> mapping dotted module paths to files (e.g., "atlas.agent" → /path/to/atlas/agent.py)
  3. At parse time, use a Tree-sitter query to find import_statement and import_from_statement nodes
  4. For each: check if it's stdlib (hardcoded set of ~70 modules), third-party (another hardcoded set), or local
  5. For local imports: resolve absolute imports via module_index lookup, resolve relative imports (.foo, ..bar) by computing the parent package path
  6. Return HashSet<PathBuf> of resolved local file paths

  JsTsImportResolver is similar but also handles TypeScript path aliases from tsconfig.json, index.js resolution, and extension-less imports.

  The CPG Overlay — Sub-File Granularity (cpg.rs, cfg.rs, dataflow.rs, callgraph.rs)

  When repo_graph.enable_cpg() is called, a second graph is created inside RepoGraph: the CpgLayer. This is a separate DiGraph<CpgNode, CpgEdge> that operates at sub-file granularity — its nodes are individual functions, methods, classes, variables, and statements within files.

  The CPG is built in 4 phases, all triggered by cpg.build_file():

  Phase 1 — AST extraction: Walk the Tree-sitter tree and create CpgNodes for each function, method, class, and module-level variable. For classes, methods are children. Each node records its name, byte range, line range, parameters (for functions), base classes (for classes), docstrings, and parent class (for methods). AstChild edges connect parents to children (file→function, class→method).

  Phase 2 — CFG construction (cfg.rs): For each function/method, CfgBuilder creates a control flow graph. It first creates two sentinel nodes:
  CfgEntry (function start) and CfgExit (function end). Then it recursively walks the function body, creating Statement nodes for each statement and connecting them with control flow edges:

  - ControlFlowNext — sequential execution
  - ControlFlowTrue/ControlFlowFalse — if/elif/while branches
  - ControlFlowBack — loop back-edges (for→header, while→header)
  - ControlFlowException — try body → except handler

  The CFG handles Python's full control flow: if/elif/else chains, for/while with else clauses, try/except/else/finally, with statements,
  match/case, break, continue, return, raise. Break statements are collected in LoopContext.break_collectors and become exits of the enclosing loop. Return/raise statements jump directly to CfgExit.

  Phase 3 — Reaching definitions analysis (dataflow.rs): For each function, DataFlowAnalyzer performs classic intra-procedural dataflow analysis:

  1. Extract def/use sets for every statement. An assignment x = expr defines x and uses all identifiers in expr. A for loop for x in items defines x and uses items. CfgEntry defines the function's parameters. The extractor handles tuple unpacking, augmented assignment (+=), with-as patterns, imports, and filters out self/cls/True/False/None.
  2. Build GEN/KILL sets. GEN(node) = the definitions at this node. KILL(node) = all other definitions of the same variables elsewhere in the function (because this assignment overwrites them).
  3. Worklist algorithm. Initialize IN/OUT sets empty. For each node: IN(n) = union of OUT(predecessors). OUT(n) = GEN(n) ∪ (IN(n) − KILL(n)). Iterate until no OUT set changes (fixpoint). The CFG's back-edges ensure loop-carried definitions propagate correctly.
  4. Create DataFlowReach edges. For each variable used at node U, find all definitions in IN(U) that match the variable name. Add a
  DataFlowReach("x") edge from the defining statement to the using statement. This tells you: "the value of x defined at line 5 reaches line 12."

  Phase 4 — Call graph (callgraph.rs): This is a two-pass architecture:

  Pass 1 (per-file, during build_file): For each function's statements, walk the Tree-sitter subtree looking for call nodes. For each call, extract: the callee name (from identifier or attribute node), the receiver (e.g., self in self.method()), positional arguments, and keyword arguments. Store as CallSite structs in cpg.call_sites[func_idx].

  Pass 2 (cross-file, after all files built): resolve_all() iterates every call site and attempts resolution:
  1. If callee is a Python builtin (print, len, range, etc. — ~50 items), skip
  2. If receiver is self, look for sibling methods in the same class
  3. If simple name, scan same-file functions for an unambiguous match
  4. If not found locally, check symbol_index.definitions[name] for cross-file matches — only resolve if exactly one match exists
  5. Otherwise, mark as Unresolved

  For each resolved call, four edge types are created:
  - Calls from caller function → callee function
  - CalledBy (reverse)
  - DataFlowArgument { position: i } from the call statement → callee's CfgEntry, one per argument (skipping self/cls parameters)
  - DataFlowReturn from each return statement in the callee → the call statement in the caller

  Incremental Updates (graph.rs update_file())

  When a file changes, Atlas doesn't rebuild the whole graph. It classifies the change by comparing hashes:

  - Local — Content hash changed but imports, definitions, and usages hashes are the same. This means only function bodies changed. Action: update the content hash. No edge changes. PageRank stays valid. If CPG is enabled, rebuild only the CPG nodes for this file.
  - FileScope — Definitions or usages hash changed (function added/removed/renamed). Action: re-harvest symbols, update the symbol index, rebuild symbol usage edges for this file and any file that references its symbols. Flag PageRank dirty.
  - GraphScope — Imports hash changed. Action: everything in FileScope plus re-resolve import edges. Previously unresolved imports in other files might now resolve to this file's new exports.

  The CPG has its own incremental path: callgraph::resolve_file() removes old inter-procedural edges for the changed file, then re-resolves call sites both within the file and from other files that reference functions with the same names.

  File Watching (watcher.rs)

  FileWatcher spawns a background thread using the notify crate with FSEvents on macOS. Events are debounced at 100ms via notify-debouncer-full. The watcher maintains a known_files set to distinguish Create from Modify (because atomic writes appear as Create events even for existing files). Events flow through a crossbeam-channel to the main thread. The FileFilter skips .git, __pycache__, node_modules, .venv, etc.

  ---
  Layer 2: The Python Shell — Orchestration and LLM Integration

  Context Assembly (context.py)

  When a user asks a question, ContextManager.assemble_context() constructs the LLM prompt using adaptive three-tier budgeting. All parameters — tier ratios, anchor counts, BFS depth, and neighborhood caps — are computed dynamically based on the target model's context window, the repository's size and graph density, and the actual measured token cost of the map.

  Model-Aware Token Budget

  The total context budget is derived from the target model's context window. A MODEL_CONTEXT_WINDOWS dictionary maps known models (Claude, GPT-4, Gemini, DeepSeek, Llama, Mistral, Qwen, etc.) to their window sizes. Atlas uses 60% of the window for context, leaving the rest for the system prompt and response. Model names are matched by exact name or prefix (e.g., "deepseek-coder:7b" matches "deepseek-coder"). Unknown models fall back to a 100K default window. An explicit max_tokens override bypasses auto-detection for backward compatibility.

  Adaptive Parameter Computation

  Before filling tiers, Atlas generates the repository map and measures its actual token cost. It then calls _compute_adaptive_params(), which reads repo_graph.get_statistics() (node_count, edge_count) and produces a ContextParams dataclass:

  Tier 1 (map): Uses the actual measured token cost of the map, capped at 8% of total budget. A small repo's 400-token map uses 400 tokens (not a wasteful 5K allocation). A large repo's map is capped but less aggressively than a fixed 5%.

  Tier 2 vs Tier 3 split: A continuous function of node_count determines the split of the remaining budget after Tier 1:
  - Tiny repos (≤30 files): ~75% full content / ~25% skeletons — most files fit as full content
  - Medium repos (~200 files): ~62% full content / ~38% skeletons — balanced
  - Large repos (500+ files): ~40% full content / ~60% skeletons — skeleton-dominant for breadth

  The formula: tier2_share = clamp(0.75 − node_count/1500, 0.40, 0.75).

  Anchor count: Scales with repo size, clamped 3–10. A 10-file repo gets 3 anchors; a 100+ file repo gets 10.

  Map ranked list: Scales with repo size, clamped 20–75. Computed as node_count // 4.

  Neighborhood BFS depth: Sparse graphs (density < 3.0 edges/node) get 3 hops; dense graphs get 2 hops. This ensures BFS reaches far enough in loosely-connected repos without exploding in highly-connected ones.

  Neighborhood file cap: Scales with repo size, clamped 10–40. Computed as node_count // 5.

  Tier Fill Process

  Tier 1: Repository map. Calls repo_graph.generate_map(max_files=map_max_files) which produces a text overview: directory structure + top-ranked files by PageRank. This gives the LLM spatial awareness of the project.

  Tier 2: Full file content, filled in priority order:
  1. Explicit files — files the caller specifies as in-scope
  2. Anchor files — embedding_manager.find_relevant_files(query, all_files, top_n=anchor_count) embeds the user's query and all file contents into vectors (using BAAI/bge-small-en-v1.5 via FastEmbed), computes cosine similarity, and returns the most similar files. Embeddings are cached per-file.
  3. Neighborhood expansion — starting from the explicit + anchor files, BFS walks the dependency graph up to neighborhood_max_hops. SymbolUsage edges get the full hop depth, Import edges get 1 hop only (to prevent transitive explosion through re-exports). Files are weighted by distance decay: hop 1 = 1.0, hop 2 = 0.5, hop 3 = 0.25. Up to neighborhood_max_files neighbors are returned.

  Tier 3: Skeletons of high-PageRank files. Takes the top 100 files by PageRank score, generates skeletons (signatures only, bodies replaced with ...), and fills remaining budget. This gives the LLM a panoramic view of the project's most architecturally significant interfaces.

  The Agent Loop (agent.py)

  AtlasAgent ties everything together. On initialize():
  1. scan_repository() walks the project directory, respecting .gitignore, collecting all source files
  2. repo_graph.build_complete(file_paths) constructs the graph (parallel parse → serial integration → edges → PageRank)
  3. FileWatcher starts monitoring for changes
  4. Top 5 files by PageRank are displayed to the user

  The agent supports two interaction modes:

  query(user_input) — single-turn:
  1. Assemble context via ContextManager
  2. Build a system prompt that instructs the LLM to use <think>...</think> for reasoning and <action>...</action> for tool calls
  3. Send to LLM (Ollama running a local model like deepseek-coder)
  4. Parse response for think blocks and action blocks
  5. If actions found, execute them via ToolExecutor

  chat(user_input, max_tool_rounds=5) — multi-turn with tool use:
  1. Append user message to conversation history
  2. Assemble context, build prompt, send to LLM
  3. Parse response — if no actions, store as assistant message and return
  4. If actions found: execute them, format results as markdown, append as a new user message ("## Tool Results"), loop back to step 2
  5. Repeat up to max_tool_rounds times

  The LLM can call four tools: read_file(path), write_file(path, content), list_directory(path), generate_repository_map(output_path).

  The Reflexive Sensory Loop (tools.py)

  When the LLM asks to write a file, ToolExecutor.write_file() doesn't just write it. First:
  1. Determine the file's language from its extension
  2. Call check_syntax(content, language) in the Rust core — for Python this uses Python's native compile() function via PyO3 (not Tree-sitter), for other languages it uses Tree-sitter
  3. If syntax is invalid, refuse the write and return the error to the LLM with status refused_by_parser
  4. The LLM sees the syntax error, fixes its code, and tries again

  This prevents the agent from ever saving syntactically broken code. If the write succeeds, a backup of the original file is created with a timestamp suffix.

  ---
  Concrete Example: A Software Engineer Using Atlas

  Scenario: You're building a Django web app. You have ~200 Python files. You're running deepseek-coder locally via Ollama. You want the LLM to help you add a new API endpoint that needs to interact with existing authentication, database models, and serialization code.

  Step 1: Start Atlas

  source .venv/bin/activate
  atlas chat -p /path/to/myproject --model deepseek-coder

  Atlas initializes:
  - scan_repository() finds 200 .py files
  - build_complete() parses all 200 files in parallel using Tree-sitter, harvests ~1,500 symbols (function/class definitions), resolves ~800 import edges and ~600 symbol usage edges
  - PageRank computes. The top files are things like models/user.py (imported by 40 files), core/auth.py (imported by 25 files), utils/db.py (imported by 30 files)
  - File watcher starts monitoring the project directory

  You see:
  Repository initialized: 200 files, 1400 edges, 1500 symbols
  Top architectural files:
    1. models/user.py (rank: 0.045)
    2. utils/db.py (rank: 0.038)
    3. core/auth.py (rank: 0.033)
    4. serializers/base.py (rank: 0.028)
    5. views/base.py (rank: 0.025)

  Step 2: Ask Your Question

  You: Add a new API endpoint POST /api/v2/teams/ that creates a team, assigns the authenticated user as owner, and returns the serialized team.

  What happens internally:

  1. Budget — ContextManager detects deepseek-coder's 128K context window and sets the total budget to ~76K tokens (60% utilization). For a 200-file repo with ~600 edges, the adaptive parameters compute: tier2_share ≈ 0.62, 10 anchor files, 2-hop BFS (density 3.0), and a 40-file neighborhood cap.
  2. Anchor — EmbeddingManager embeds your query and all 200 files' content. Cosine similarity finds the 10 most textually relevant, including: views/teams.py, models/team.py, serializers/team.py, urls/api_v2.py, tests/test_teams.py.
  3. Expand — BFS walks the dependency graph from the anchor files:
    - views/teams.py imports models/team.py, core/auth.py, serializers/team.py → all pulled in (hop 1, weight 1.0)
    - models/team.py imports models/user.py, utils/db.py → pulled in (hop 2, weight 0.5)
    - serializers/team.py imports serializers/base.py → pulled in (hop 2, weight 0.5)
    - Import-only edges stop at 1 hop, so transitive re-exports don't explode the neighborhood
  4. Budget fill — The context window gets populated with adaptive tier sizes:
    - Tier 1 (~2K tokens): Repo map (actual measured cost, well under the 8% cap) showing directory structure and top-ranked files
    - Tier 2 (~46K tokens): Full content of views/teams.py, models/team.py, serializers/team.py, urls/api_v2.py, core/auth.py, models/user.py, utils/db.py, serializers/base.py — roughly 12 files with full source
    - Tier 3 (~28K tokens): Skeletons of the next ~60 highest-ranked files — just signatures, no implementations. The LLM can see that
  utils/permissions.py has def check_team_permission(user, team, action) without seeing its 50-line body
  4. LLM call — The model receives a prompt with the system instructions, all that context, and your question. It has everything it needs: the existing team model schema, the auth decorators used in other views, the base serializer class, the URL routing pattern, and the signatures of permission utilities.
  5. Response parsing — The LLM responds with <think> (reasoning about the existing patterns) and <action> blocks (tool calls):

  <think>
  Looking at views/teams.py, existing endpoints use @login_required and TeamSerializer. The URL pattern in urls/api_v2.py uses
  path('teams/', TeamViewSet.as_view(...)). I need to add a create method to the existing viewset.
  </think>
  <action>
  write_file("views/teams.py", "...updated code...")
  </action>
  6. Reflexive Sensory Loop — Before writing, check_syntax() compiles the Python code. If the LLM forgot a colon or had a syntax error, the write is refused and the error is returned. The LLM fixes it and tries again.
  7. Graph update — The FileWatcher detects the change to views/teams.py. The agent calls repo_graph.update_file(). Atlas classifies the change:
    - If you only added a method to an existing class: Local tier — content hash changed but definitions/imports didn't. Quick update.
    - If the new code imports a new module: GraphScope tier — import edges recalculated, PageRank flagged dirty.

  Step 3: Continue the Conversation

  You: Now add a test for this endpoint

  The conversation history is preserved. The context manager re-runs Anchor & Expand with this new query — this time tests/test_teams.py will rank higher as an anchor. The LLM sees the existing test patterns, the newly written endpoint code (updated in the graph), and generates a test that follows the project's conventions.

  What makes this different from just handing files to an LLM:

  Without Atlas, you'd have to manually figure out which of your 200 files the LLM needs to see. You'd probably paste 3-4 files and miss the base serializer class, or forget the auth decorator pattern, and the LLM would generate code that doesn't match your project's conventions.

  Atlas automates this entirely. The graph knows core/auth.py is structurally central. The vector search knows your query is about teams. The BFS expansion follows real import/usage edges to pull in the exact dependency chain. The skeleton tier gives the LLM passive awareness of dozens of additional files without burning context tokens on implementation details. The adaptive budgeting ensures these ratios make sense whether you have 20 files or 2,000, and whether you're using a 128K local model or a 1M-token cloud model. And the syntax validation loop catches mistakes before they hit disk.

  The engineer never thinks about context management. They just ask questions and the system figures out what the LLM needs to see.
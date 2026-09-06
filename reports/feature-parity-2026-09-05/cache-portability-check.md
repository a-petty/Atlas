# Embedding cache portability check

Independent read-only inspection and tiny pure helper checks confirmed that identical accepted skeleton text, repository-relative path and engine identity produce the same persistent embedding input/key across unrelated checkout roots for Python, JavaScript, TypeScript and TSX.

`embeddings.py` derives a relative dotted prefix, prepends it to graph skeleton text, and hashes the engine identity plus that text. Native skeletonization consumes source, tree and language without an absolute-path header. Absolute paths remain in transient file/matrix mappings, which rebuild per checkout.

This check found no language-specific root leakage in persistent keys. It did not broadly compare Material UI snapshots or determine which source/path changes explain their new entries. It used no model initialization, real repository indexing or source edits. Existing restart, rename and model-identity tests remain the formal cache regressions.

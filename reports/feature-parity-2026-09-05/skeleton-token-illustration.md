# Single-file skeleton token illustration

This is a deliberately selected illustration using Atlas's implementation-heavy `python_shell/atlas/session.py`, not a frozen benchmark, a representative compression estimate, or a measurement of whole-agent savings.

The direct parser accepted captured UTF-8 source bytes and returned a skeleton without an embedding model, repository index, graph build or MCP worker. Counts use `cl100k_base` from tiktoken 0.14.0 in `work/locked-venv`.

| Representation | Tokens |
| --- | ---: |
| Full source text | 5,569 |
| Plain skeleton text | 603 |
| Raw structured parser response, including duplicated chunk text and spans | 2,713 |

The plain skeleton removes **4,966 tokens (89.2%)** from this one file. It contains 30 chunks and retains 2,502 unique original source bytes from the 27,159-byte file. The direct string API and the structured API's text field were verified equal, and every retained span was checked against the captured bytes. The raw parser JSON figure is shown to make representation overhead visible; it is not the MCP context format, whose manifest avoids duplicating chunk source.

The reduction comes with information loss. The skeleton omits all three of these implementation checks:

- Line 195: the second manifest/configuration comparison that rejects edits occurring during source capture.
- Line 440: the cancellation-ownership check that decides whether a cancellation may terminate the active worker.
- Line 434: the fallback kill-and-join behavior used when termination does not stop the worker.

The skeleton can help a model locate the relevant methods. It cannot establish whether these behaviors are correct. If the model subsequently reads the whole file anyway, the two content payloads total 6,172 tokens before wrappers or other tool costs, versus 5,569 for starting with the full file. A targeted follow-up read could still preserve savings; this illustration does not measure a model choosing between those workflows.

Exact source, skeleton, structured parser response, retained spans, omitted examples and provenance are preserved in `skeleton-token-illustration.json`. The source was clean at commit `d90e59d90701bc9059385e80f9c626dca77f846a`; its SHA256 is `ef0da19b783dcdb9d7df6b824ea335d570c7e319c2429316440f982d0ed24fbb`. The loaded release extension SHA256 is `80a276b6d0bbfba972a408a702515916236ccbdd9c56672f0a61788a511ba3a5`.

These text counts exclude MCP schemas, call arguments, coverage/pagination wrappers, conversation history, reasoning, generated output and follow-up reads. They are specific to this tokenizer. No live source was edited and no general accuracy or token-cost gain is inferred.

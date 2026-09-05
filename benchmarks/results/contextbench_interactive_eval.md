# ContextBench Interactive Evaluation

Claude Code + Atlas MCP performing context retrieval on 10 ContextBench tasks.
This evaluates the real-world workflow: Claude reads the issue, uses Atlas tools
to explore the repo, and identifies the relevant files.

## Scoring

For each task:
- **Gold files**: human-annotated files needed to resolve the issue
- **Retrieved files**: files identified by Claude Code + Atlas
- **Recall**: gold files found / total gold files
- **Precision**: gold files found / total retrieved files
- **F1**: harmonic mean

---

## Task 1: django/django — QuerySet.only() crash on proxy models

**Issue**: `QuerySet.only()` after `select_related()` crashes on proxy models.
**Gold**: `django/db/models/sql/query.py`

**Retrieved**: (pending)
**Recall**: —
**Precision**: —
**F1**: —

---

## Task 2: django/django — Use stdlib html.escape()

**Issue**: Replace `django.utils.html.escape()` with Python stdlib `html.escape()`.
**Gold**: `django/utils/html.py`

**Retrieved**: (pending)
**Recall**: —
**Precision**: —
**F1**: —

---

## Task 3: sympy/sympy — is_zero incorrect on complex integer

**Issue**: `is_zero` returns `False` on `-2*I + (1 + I)**2` which is actually zero.
**Gold**: `sympy/core/add.py`, `sympy/core/tests/test_arit.py`, `sympy/core/tests/test_assumptions.py`

**Retrieved**: (pending)
**Recall**: —
**Precision**: —
**F1**: —

---

## Task 4: sphinx-doc/sphinx — viewcode creates epub pages incorrectly

**Issue**: viewcode creates pages for epub even if `viewcode_enable_epub=False`.
**Gold**: `sphinx/ext/viewcode.py`

**Retrieved**: (pending)
**Recall**: —
**Precision**: —
**F1**: —

---

## Task 5: scikit-learn/scikit-learn — IterativeImputer missing fill_value

**Issue**: `IterativeImputer` doesn't pass `fill_value` to its internal `SimpleImputer`.
**Gold**: `sklearn/impute/_base.py`, `sklearn/impute/_iterative.py`

**Retrieved**: (pending)
**Recall**: —
**Precision**: —
**F1**: —

---

## Task 6: django/django — Nested class methods can't be Field.default

**Issue**: Class methods from nested classes cannot be used as `Field.default`.
**Gold**: `django/db/migrations/serializer.py`

**Retrieved**: (pending)
**Recall**: —
**Precision**: —
**F1**: —

---

## Task 7: django/django — Exists subquery with empty queryset removes WHERE

**Issue**: Filter on exists-subquery with empty queryset removes the whole WHERE block.
**Gold**: `django/db/models/expressions.py`, `django/db/models/sql/query.py`

**Retrieved**: (pending)
**Recall**: —
**Precision**: —
**F1**: —

---

## Task 8: django/django — method_decorator() should preserve wrapper assignments

**Issue**: `method_decorator()` doesn't preserve `__name__`, `__module__` etc.
**Gold**: `django/utils/decorators.py`

**Retrieved**: (pending)
**Recall**: —
**Precision**: —
**F1**: —

---

## Task 9: huggingface/transformers — TFBertTokenizer with TF text

**Issue**: Allow `TFBertTokenizer` to use TF text `BertTokenizer` for TF Serving.
**Gold**: `src/transformers/models/bert/__init__.py`, `src/transformers/models/bert/tokenization_bert_tf.py`, `src/transformers/utils/dummy_tensorflow_text_objects.py`, `src/transformers/utils/import_utils.py`, `tests/models/bert/test_tokenization_bert_tf.py`

**Retrieved**: (pending)
**Recall**: —
**Precision**: —
**F1**: —

---

## Task 10: django/django — Add DISTINCT support for Avg and Sum

**Issue**: Add DISTINCT support for `Avg` and `Sum` aggregates.
**Gold**: `django/db/models/aggregates.py`

**Retrieved**: (pending)
**Recall**: —
**Precision**: —
**F1**: —

---

## Aggregate Results

| Task | Repo | Gold | Retrieved | Recall | Precision | F1 |
|------|------|------|-----------|--------|-----------|-----|
| 1 | django | 1 | — | — | — | — |
| 2 | django | 1 | — | — | — | — |
| 3 | sympy | 3 | — | — | — | — |
| 4 | sphinx | 1 | — | — | — | — |
| 5 | scikit-learn | 2 | — | — | — | — |
| 6 | django | 1 | — | — | — | — |
| 7 | django | 2 | — | — | — | — |
| 8 | django | 1 | — | — | — | — |
| 9 | transformers | 5 | — | — | — | — |
| 10 | django | 1 | — | — | — | — |
| **Mean** | | | | — | — | — |

# ContextBench Evaluation Prompt

## Instructions for running

Each task requires Atlas MCP to be pointed at a different repo checkout. Start a separate Claude Code session for each task, launched from the repo worktree directory listed below.

For each task:

```bash
cd <worktree_path>
claude
```

Then paste the evaluation prompt for that task into the session.

Record the results in `benchmarks/results/contextbench_interactive_eval.md` after all tasks are complete.

---

## Task 1

**Worktree:** `/tmp/contextbench_repos/django/django/worktrees/5eb6a2b33d`

**Prompt to paste:**

```
I'm evaluating the Atlas MCP context retrieval system. You have Atlas MCP tools available. Use them to investigate the following GitHub issue and identify which source files would need to be modified to fix it. Use Atlas tools like find_relevant_files, get_repository_map, get_dependencies, get_dependents, assemble_context, and get_file_symbols to explore. Work through it naturally as you would in a real coding session.

When you're done, list your final answer as a JSON array of file paths relative to the repo root.

IMPORTANT: Do NOT look at git history, git log, or git blame. Do NOT look at the test suite to find the answer. Pretend this bug has not been fixed yet. You are trying to locate the source files that would need to be changed.

GitHub Issue:

QuerySet.only() after select_related() crash on proxy models.

When I optimize a query using select_related() and only() methods from the proxy model I encounter an error:
Windows 10; Python 3.10; Django 4.0.5

Traceback (most recent call last):
  File "...\django\db\models\query.py", line 302, in __len__
    self._fetch_all()
  File "...\django\db\models\query.py", line 1507, in _fetch_all
    self._result_cache = list(self._iterable_class(self))
  File "...\django\db\models\query.py", line 71, in __iter__
    related_populators = get_related_populators(klass_info, select, db)
  File "...\django\db\models\query.py", line 2268, in get_related_populators
    rel_cls = RelatedPopulator(rel_klass_info, select, db)
  File "...\django\db\models\query.py", line 2243, in __init__
    self.pk_idx = self.init_list.index(self.model_cls._meta.pk.attname)
ValueError: 'id' is not in list
```

---

## Task 2

**Worktree:** `/tmp/contextbench_repos/django/django/worktrees/28d5262fa3`

**Prompt to paste:**

```
I'm evaluating the Atlas MCP context retrieval system. You have Atlas MCP tools available. Use them to investigate the following GitHub issue and identify which source files would need to be modified to fix it. Use Atlas tools like find_relevant_files, get_repository_map, get_dependencies, get_dependents, assemble_context, and get_file_symbols to explore. Work through it naturally as you would in a real coding session.

When you're done, list your final answer as a JSON array of file paths relative to the repo root.

IMPORTANT: Do NOT look at git history, git log, or git blame. Do NOT look at the test suite to find the answer. Pretend this bug has not been fixed yet. You are trying to locate the source files that would need to be changed.

GitHub Issue:

Use Python stdlib html.escape() to in django.utils.html.escape()

The function django.utils.html.escape() partially duplicates the Python stdlib function html.escape(). We can replace this duplication with wider community developed version.

html.escape() has been available since Python 3.2: https://docs.python.org/3/library/html.html#html.escape

This function is also faster than Django's. As Python bug https://bugs.python.org/issue18020 concludes, using .replace() can be faster than .translate(). This function gets called numerous times when rendering templates. After making the change locally, I saw the following improvement:

master:
$ python -m timeit -s 'from django.utils.html import escape' 'escape(copyright)'
50000 loops, best of 5: 4.03 usec per loop

branch:
$ python -m timeit -s 'from django.utils.html import escape' 'escape(copyright)'
100000 loops, best of 5: 2.45 usec per loop

One small concern, html.escape() converts ' to &#x27 rather than &#39. These values are functionally equivalent HTML, but I'll mention it as a backwards incompatible change as the literal text has changed.
```

---

## Task 3

**Worktree:** `/tmp/contextbench_repos/sympy/sympy/worktrees/b506169ad7`

**Prompt to paste:**

```
I'm evaluating the Atlas MCP context retrieval system. You have Atlas MCP tools available. Use them to investigate the following GitHub issue and identify which source files would need to be modified to fix it. Use Atlas tools like find_relevant_files, get_repository_map, get_dependencies, get_dependents, assemble_context, and get_file_symbols to explore. Work through it naturally as you would in a real coding session.

When you're done, list your final answer as a JSON array of file paths relative to the repo root.

IMPORTANT: Do NOT look at git history, git log, or git blame. Do NOT look at the test suite to find the answer. Pretend this bug has not been fixed yet. You are trying to locate the source files that would need to be changed. Test files ARE acceptable in your answer if they need to be updated too.

GitHub Issue:

is_zero is incorrect on complex integer

`is_zero` should return `None` if it cannot decide, but should never give the wrong answer. However:

```python
>>> e = -2*I + (1 + I)**2
>>> e.is_zero
False
>>> simplify(e).is_zero
True
```

This is causing errors in determining the rank of a matrix. See issue #15872.

Fix is_zero for complex numbers while Add.

References: #15873

Release notes:
- core: Fix `is_zero` becoming `False` on some expressions with `Add`.
```

---

## Task 4

**Worktree:** `/tmp/contextbench_repos/sphinx-doc/sphinx/worktrees/82ef497a8c`

**Prompt to paste:**

```
I'm evaluating the Atlas MCP context retrieval system. You have Atlas MCP tools available. Use them to investigate the following GitHub issue and identify which source files would need to be modified to fix it. Use Atlas tools like find_relevant_files, get_repository_map, get_dependencies, get_dependents, assemble_context, and get_file_symbols to explore. Work through it naturally as you would in a real coding session.

When you're done, list your final answer as a JSON array of file paths relative to the repo root.

IMPORTANT: Do NOT look at git history, git log, or git blame. Do NOT look at the test suite to find the answer. Pretend this bug has not been fixed yet. You are trying to locate the source files that would need to be changed.

GitHub Issue:

viewcode creates pages for epub even if `viewcode_enable_epub=False` on `make html epub`

**Describe the bug**
viewcode creates pages for epub even if `viewcode_enable_epub=False` on `make html epub`

**To Reproduce**
```
$ make html epub
```

**Expected behavior**
module pages should not be created for epub by default.

**Environment info**
- OS: Mac
- Python version: 3.9.1
- Sphinx version: HEAD of 3.x
- Sphinx extensions: sphinx.ext.viewcode
```

---

## Task 5

**Worktree:** `/tmp/contextbench_repos/scikit-learn/scikit-learn/worktrees/f7eea97809`

**Prompt to paste:**

```
I'm evaluating the Atlas MCP context retrieval system. You have Atlas MCP tools available. Use them to investigate the following GitHub issue and identify which source files would need to be modified to fix it. Use Atlas tools like find_relevant_files, get_repository_map, get_dependencies, get_dependents, assemble_context, and get_file_symbols to explore. Work through it naturally as you would in a real coding session.

When you're done, list your final answer as a JSON array of file paths relative to the repo root.

IMPORTANT: Do NOT look at git history, git log, or git blame. Do NOT look at the test suite to find the answer. Pretend this bug has not been fixed yet. You are trying to locate the source files that would need to be changed.

GitHub Issue:

IterativeImputer has no parameter "fill_value"

In the first imputation round of `IterativeImputer`, an initial value needs to be set for the missing values. From its docs:

> **initial_strategy {'mean', 'median', 'most_frequent', 'constant'}, default='mean'**
> Which strategy to use to initialize the missing values. Same as the strategy parameter in SimpleImputer.

I have set the initial strategy to `"constant"`. However, I want to define this constant myself. So, as I look at the parameters for `SimpleImputer` I find `fill_value`:

> When strategy == "constant", fill_value is used to replace all occurrences of missing_values. If left to the default, fill_value will be 0 when imputing numerical data and "missing_value" for strings or object data types.

Based on this information, one would assume that `IterativeImputer` also has the parameter `fill_value`, but it does not.

The parameter `fill_value` needs to be added to `IterativeImputer` for when `initial_strategy` is set to `"constant"`. If this parameter is added, please also allow `np.nan` as `fill_value`, for optimal compatibility with decision tree-based estimators.
```

---

## Task 6

**Worktree:** `/tmp/contextbench_repos/django/django/worktrees/4a72da7100`

**Prompt to paste:**

```
I'm evaluating the Atlas MCP context retrieval system. You have Atlas MCP tools available. Use them to investigate the following GitHub issue and identify which source files would need to be modified to fix it. Use Atlas tools like find_relevant_files, get_repository_map, get_dependencies, get_dependents, assemble_context, and get_file_symbols to explore. Work through it naturally as you would in a real coding session.

When you're done, list your final answer as a JSON array of file paths relative to the repo root.

IMPORTANT: Do NOT look at git history, git log, or git blame. Do NOT look at the test suite to find the answer. Pretend this bug has not been fixed yet. You are trying to locate the source files that would need to be changed.

GitHub Issue:

Class methods from nested classes cannot be used as Field.default.

Given the following model:

class Profile(models.Model):
    class Capability(models.TextChoices):
        BASIC = ("BASIC", "Basic")
        PROFESSIONAL = ("PROFESSIONAL", "Professional")

        @classmethod
        def default(cls) -> list[str]:
            return [cls.BASIC]
    capabilities = ArrayField(
        models.CharField(choices=Capability.choices, max_length=30, blank=True),
        null=True,
        default=Capability.default
    )

The resulting migration contained the following:
    migrations.AddField(
        model_name='profile',
        name='capabilities',
        field=django.contrib.postgres.fields.ArrayField(..., default=appname.models.Capability.default, ...),
    ),

As you can see, migrations.AddField is passed as argument "default" a wrong value "appname.models.Capability.default", which leads to an error when trying to migrate. The right value should be "appname.models.Profile.Capability.default".
```

---

## Task 7

**Worktree:** `/tmp/contextbench_repos/django/django/worktrees/770d3e6a4c`

**Prompt to paste:**

```
I'm evaluating the Atlas MCP context retrieval system. You have Atlas MCP tools available. Use them to investigate the following GitHub issue and identify which source files would need to be modified to fix it. Use Atlas tools like find_relevant_files, get_repository_map, get_dependencies, get_dependents, assemble_context, and get_file_symbols to explore. Work through it naturally as you would in a real coding session.

When you're done, list your final answer as a JSON array of file paths relative to the repo root.

IMPORTANT: Do NOT look at git history, git log, or git blame. Do NOT look at the test suite to find the answer. Pretend this bug has not been fixed yet. You are trying to locate the source files that would need to be changed.

GitHub Issue:

filter on exists-subquery with empty queryset removes whole WHERE block

>>> qs = MyModel.objects.filter(~models.Exists(MyModel.objects.none()), name='test')
>>> qs
<QuerySet []>
>>> print(qs.query)
EmptyResultSet

With django-debug-toolbar I can still see the query, but the WHERE block is missing completely.

This seems to be very similar to #33018.
```

---

## Task 8

**Worktree:** `/tmp/contextbench_repos/django/django/worktrees/004b4620f6`

**Prompt to paste:**

```
I'm evaluating the Atlas MCP context retrieval system. You have Atlas MCP tools available. Use them to investigate the following GitHub issue and identify which source files would need to be modified to fix it. Use Atlas tools like find_relevant_files, get_repository_map, get_dependencies, get_dependents, assemble_context, and get_file_symbols to explore. Work through it naturally as you would in a real coding session.

When you're done, list your final answer as a JSON array of file paths relative to the repo root.

IMPORTANT: Do NOT look at git history, git log, or git blame. Do NOT look at the test suite to find the answer. Pretend this bug has not been fixed yet. You are trying to locate the source files that would need to be changed.

GitHub Issue:

method_decorator() should preserve wrapper assignments

The function that is passed to the decorator is a partial object and does not have any of the attributes expected from a function i.e. __name__, __module__ etc...

Consider the following case:

def logger(func):
    @wraps(func)
    def inner(*args, **kwargs):
        try:
            result = func(*args, **kwargs)
        except Exception as e:
            result = str(e)
        finally:
            logger.debug(f"{func.__name__} called with args: {args} and kwargs: {kwargs} resulting: {result}")
    return inner

class Test:
    @method_decorator(logger)
    def hello_world(self):
        return "hello"

Test().test_method()

This results in the following exception:
AttributeError: 'functools.partial' object has no attribute '__name__'
```

---

## Task 9

**Worktree:** `/tmp/contextbench_repos/huggingface/transformers/worktrees/3d320c78c3`

**Prompt to paste:**

```
I'm evaluating the Atlas MCP context retrieval system. You have Atlas MCP tools available. Use them to investigate the following GitHub issue and identify which source files would need to be modified to implement this feature. Use Atlas tools like find_relevant_files, get_repository_map, get_dependencies, get_dependents, assemble_context, and get_file_symbols to explore. Work through it naturally as you would in a real coding session.

When you're done, list your final answer as a JSON array of file paths relative to the repo root. Include test files if they'd need to be created or modified.

IMPORTANT: Do NOT look at git history, git log, or git blame. Do NOT look at the test suite to find the answer. Pretend this feature has not been implemented yet. You are trying to locate the source files that would need to be changed.

GitHub Issue:

Allow TFBertTokenizer to use Tensorflow text BertTokenizer (and not FastBertTokenizer) to make it servable by TF Serving

I would like to serve a bundle of Tokenizer + Model on TF Serving, but can't do it because TF Serving still have no support for TF FastBertTokenizer and FastBertNormalize operations (https://github.com/tensorflow/serving/issues/2064).

It would be good if we could let TFBertTokenizer give the user an option not to use Tensorflow FastBertTokenizer when creating a TFBertTokenizer, so that it is servable on TFServing.

It would consist of moving (or creating an option to change) the fast tokenizer to use the standard tensorflow_text BertTokenizer instead:

```python
from tensorflow_text import BertTokenizer as TFBertTokenizerLayer

lookup_table = tf.lookup.StaticVocabularyTable(
    tf.lookup.KeyValueTensorInitializer(
        keys=vocab_list,
        key_dtype=tf.string,
        values=tf.range(
            tf.size(vocab_list, out_type=tf.int64), dtype=tf.int64),
            value_dtype=tf.int64
        ),
        num_oov_buckets=1
)

self.tf_tokenizer = TFBertTokenizerLayer(
    lookup_table, token_out_type=tf.int64, lower_case=do_lower_case
)
```
```

---

## Task 10

**Worktree:** `/tmp/contextbench_repos/django/django/worktrees/f618e033ac`

**Prompt to paste:**

```
I'm evaluating the Atlas MCP context retrieval system. You have Atlas MCP tools available. Use them to investigate the following GitHub issue and identify which source files would need to be modified to fix it. Use Atlas tools like find_relevant_files, get_repository_map, get_dependencies, get_dependents, assemble_context, and get_file_symbols to explore. Work through it naturally as you would in a real coding session.

When you're done, list your final answer as a JSON array of file paths relative to the repo root.

IMPORTANT: Do NOT look at git history, git log, or git blame. Do NOT look at the test suite to find the answer. Pretend this bug has not been fixed yet. You are trying to locate the source files that would need to be changed.

GitHub Issue:

Add DISTINCT support for Avg and Sum aggregates.

As an extension of #28658, aggregates should be supported for other general aggregates such as Avg and Sum. Before 2.2, these aggregations just ignored the parameter, but now throw an exception.

This change would just involve setting these classes as allowing DISTINCT, and could also be applied to Min and Max (although pointless).
```

---

## Scoring (after all tasks)

Record each session's final answer and score against these gold files:

| Task | Repo | Gold Files |
|------|------|------------|
| 1 | django | `django/db/models/sql/query.py` |
| 2 | django | `django/utils/html.py` |
| 3 | sympy | `sympy/core/add.py`, `sympy/core/tests/test_arit.py`, `sympy/core/tests/test_assumptions.py` |
| 4 | sphinx | `sphinx/ext/viewcode.py` |
| 5 | scikit-learn | `sklearn/impute/_base.py`, `sklearn/impute/_iterative.py` |
| 6 | django | `django/db/migrations/serializer.py` |
| 7 | django | `django/db/models/expressions.py`, `django/db/models/sql/query.py` |
| 8 | django | `django/utils/decorators.py` |
| 9 | transformers | `src/transformers/models/bert/__init__.py`, `src/transformers/models/bert/tokenization_bert_tf.py`, `src/transformers/utils/dummy_tensorflow_text_objects.py`, `src/transformers/utils/import_utils.py`, `tests/models/bert/test_tokenization_bert_tf.py` |
| 10 | django | `django/db/models/aggregates.py` |

**Metrics per task:**
- Recall = (gold files found) / (total gold files)
- Precision = (gold files found) / (total files in answer)
- F1 = 2 * recall * precision / (recall + precision)

**Paper baselines to compare against:**
- Best file F1: 0.634 (mini-SWE-Agent + GPT-5)
- Best file recall: 0.733 (OpenHands + GPT-5)

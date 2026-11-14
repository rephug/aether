# ECMAScript ownership identity scope

The parser gives declarations a syntactic lexical owner and rejects a file when
two declarations still have the same stable ID. Rejection is intentional: callers
can quarantine the file and report it, instead of silently overwriting a symbol.

## Regression corpus

The executable corpus is in crates/aether-parse/src/parser_tests.rs and
parser_ecmascript_tests.rs and languages/typescript_owners.rs. It covers:

- named factories and returned object methods;
- conditional/switch/try branches, body-free loop headers, callbacks, repeated call sites and expression arrows;
- array element owners and comment-independent element/argument ordinals;
- literal-preserving call labels and quoted/computed object property keys;
- ordinary/augmented assignment LHS owners and named class expressions;
- ordinary, static, instance, private and computed class fields;
- static/instance methods and getter/setter ancestor owners;
- body, line-offset and comment stability, plus source-edge identity linkage;
- lexical function hoisting, recursion, sibling calls and global namesakes;
- parameter/destructuring/rest/default, variable, catch and loop shadow barriers;
- default-value object owners and destructuring-pattern body stability;
- generator/static-block isolation and separate parameter/body environments;
- switch-case declarations and function/class constructor targets;
- extraction-local occurrence-cache operation counts and cross-file reset.

## Explicit limits

This is not runtime evaluation or universal identity stability. Computed keys
are represented by their syntax: keyA and keyB are distinguishable expressions,
but the parser does not claim their runtime values differ. Dynamic member dispatch,
assignment dataflow and legacy Annex B scope behavior are not inferred.

Repeated assignments to the same LHS, repeated identical dynamic computed owners,
same-named bindings in unrepresented sibling block scopes, and namespace-qualified
declarations and JSX attribute-owned methods are not universally distinguished. Representative collisions are
tested to fail closed, rather than dropping one definition. Identical call-site
or array-element insertions may renumber anonymous positional identities.

Generator symbol extraction and dynamic/positional scope resolution are not claimed complete.

The occurrence cache removes repeated rescans for declarations sharing a lexical
scope. It does not promise linear complexity for arbitrary deeply nested ASTs.

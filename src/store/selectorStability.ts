// The rule every `useAppStore` selector has to keep: asked twice about the same
// state, it returns the same value. React re-reads a store selector after each
// render and renders again when the answer differs, so a selector that builds
// its result — `s.x ?? []`, `s.list.filter(…)`, `{ a: s.a }` — never settles,
// and the component dies with "Maximum update depth exceeded" (React error
// #185, minified in a release build).
//
// `useShallow` moves the rule one level down: the collection may be new, but
// each of its elements must be the same value as last time, so an element
// built in the selector fails the same way.
//
// This finds the shapes that break the rule in a file's source. It reads
// syntax, not types, so it says nothing about a selector passed by name or a
// result that comes out of a function call. Where a method is both an array's
// and a string's (`slice`, `concat`) it is read as the array's, which builds
// one; a string says so by being spelled as one (`String(s.id).slice(0, 8)`).
import ts from "typescript";

export interface UnstableSelector {
  /** 1-indexed line of the expression that builds the value. */
  line: number;
  message: string;
}

const STORE_HOOKS = new Set(["useAppStore"]);
const SHALLOW_HOOKS = new Set(["useShallow"]);

/** Array methods that return a new array whatever they are given. */
const BUILDS_ARRAY = new Set([
  "map",
  "filter",
  "flatMap",
  "flat",
  "toSorted",
  "toReversed",
  "toSpliced",
  "split",
]);
/** Methods arrays and strings share: a new array from an array, a value from
 *  a string. Read as the array's unless the receiver `isSpelledAsString`. */
const ARRAY_OR_STRING = new Set(["slice", "concat"]);
/** Methods that hand back the array they were called on, so the result is new
 *  exactly when that array is: `[...s.items].sort()`. */
const RETURNS_RECEIVER = new Set(["sort", "reverse", "fill"]);
const OBJECT_BUILDERS = new Set(["keys", "values", "entries", "fromEntries"]);

type Selector = ts.ArrowFunction | ts.FunctionExpression;

const isSelector = (node: ts.Node | undefined): node is Selector =>
  node !== undefined && (ts.isArrowFunction(node) || ts.isFunctionExpression(node));

const calls = (node: ts.Node, names: Set<string>): node is ts.CallExpression =>
  ts.isCallExpression(node) && ts.isIdentifier(node.expression) && names.has(node.expression.text);

function unwrap(node: ts.Expression): ts.Expression {
  let e = node;
  while (
    ts.isParenthesizedExpression(e) ||
    ts.isAsExpression(e) ||
    ts.isNonNullExpression(e) ||
    ts.isSatisfiesExpression(e)
  ) {
    e = e.expression;
  }
  return e;
}

/** A string by its spelling alone: a literal, a template, a `String(…)` call. */
function isSpelledAsString(node: ts.Expression): boolean {
  const e = unwrap(node);
  return (
    ts.isStringLiteralLike(e) ||
    ts.isTemplateExpression(e) ||
    (ts.isCallExpression(e) && ts.isIdentifier(e.expression) && e.expression.text === "String")
  );
}

interface Built {
  at: ts.Expression;
  what: string;
  /** What to do when the reading is wrong, for the shapes syntax can't settle. */
  unless?: string;
}

/** What `fn` can return: its expression body, or every `return` of its block
 *  (not those of functions nested in it). */
function returnsOf(fn: Selector): ts.Expression[] {
  if (!ts.isBlock(fn.body)) return [fn.body];
  const found: ts.Expression[] = [];
  const visit = (node: ts.Node) => {
    if (ts.isFunctionLike(node)) return;
    if (ts.isReturnStatement(node) && node.expression) found.push(node.expression);
    ts.forEachChild(node, visit);
  };
  ts.forEachChild(fn.body, visit);
  return found;
}

/** The `const` initializers declared directly in `fn`'s block, by name, so a
 *  value built on one line and returned on the next is still seen. */
function localsOf(fn: Selector): Map<string, ts.Expression> {
  const locals = new Map<string, ts.Expression>();
  if (!ts.isBlock(fn.body)) return locals;
  for (const statement of fn.body.statements) {
    if (!ts.isVariableStatement(statement)) continue;
    if (!(statement.declarationList.flags & ts.NodeFlags.Const)) continue;
    for (const d of statement.declarationList.declarations) {
      if (ts.isIdentifier(d.name) && d.initializer) locals.set(d.name.text, d.initializer);
    }
  }
  return locals;
}

/** The expression inside `node` that builds a new object or array each time it
 *  runs, with what it is; undefined when nothing here does. */
function built(node: ts.Expression, locals: Map<string, ts.Expression>): Built | undefined {
  const e = unwrap(node);
  if (ts.isArrayLiteralExpression(e)) return { at: e, what: "array" };
  if (ts.isObjectLiteralExpression(e)) return { at: e, what: "object" };
  if (ts.isNewExpression(e)) return { at: e, what: "instance" };
  if (ts.isIdentifier(e)) {
    const init = locals.get(e.text);
    return init && built(init, locals);
  }
  if (ts.isConditionalExpression(e)) {
    return built(e.whenTrue, locals) ?? built(e.whenFalse, locals);
  }
  if (ts.isBinaryExpression(e)) {
    const op = e.operatorToken.kind;
    const picksASide =
      op === ts.SyntaxKind.QuestionQuestionToken ||
      op === ts.SyntaxKind.BarBarToken ||
      op === ts.SyntaxKind.AmpersandAmpersandToken;
    return picksASide ? (built(e.left, locals) ?? built(e.right, locals)) : undefined;
  }
  if (ts.isCallExpression(e) && ts.isPropertyAccessExpression(e.expression)) {
    const { expression: on, name } = e.expression;
    const onGlobal = (global: string) => ts.isIdentifier(on) && on.text === global;
    if (onGlobal("Object") && OBJECT_BUILDERS.has(name.text)) return { at: e, what: "array" };
    if (onGlobal("Array") && name.text === "from") return { at: e, what: "array" };
    if (BUILDS_ARRAY.has(name.text)) return { at: e, what: "array" };
    if (ARRAY_OR_STRING.has(name.text) && !isSpelledAsString(on)) {
      return { at: e, what: "array", unless: "if it is a string, call it on `String(…)`" };
    }
    if (RETURNS_RECEIVER.has(name.text)) return built(on, locals);
  }
  return undefined;
}

/** The expressions a `useShallow` selector's result is made of, where its
 *  shape says so: an array's elements, an object's values, what a `.map`
 *  callback returns. */
function elementsOf(result: ts.Expression): { of: ts.Expression; in: Selector | undefined }[] {
  const e = unwrap(result);
  if (ts.isArrayLiteralExpression(e)) {
    return e.elements
      .filter((el) => !ts.isSpreadElement(el))
      .map((el) => ({ of: el, in: undefined }));
  }
  if (ts.isObjectLiteralExpression(e)) {
    return e.properties
      .filter(ts.isPropertyAssignment)
      .map((p) => ({ of: p.initializer, in: undefined }));
  }
  if (
    ts.isCallExpression(e) &&
    ts.isPropertyAccessExpression(e.expression) &&
    e.expression.name.text === "map"
  ) {
    const each = e.arguments[0];
    if (isSelector(each)) return returnsOf(each).map((r) => ({ of: r, in: each }));
  }
  return [];
}

export function unstableSelectors(fileName: string, source: string): UnstableSelector[] {
  const file = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true);
  const found: UnstableSelector[] = [];
  const report = (hit: Built, why: string) => {
    const text = hit.at.getText(file).replace(/\s+/g, " ");
    const shown = text.length > 80 ? `${text.slice(0, 80)}…` : text;
    const unless = hit.unless ? ` (${hit.unless})` : "";
    found.push({
      line: file.getLineAndCharacterOfPosition(hit.at.getStart(file)).line + 1,
      message: `${why} \`${shown}\` is a new ${hit.what} on every call${unless}`,
    });
  };

  const visit = (node: ts.Node) => {
    if (calls(node, STORE_HOOKS)) {
      const selector = node.arguments[0];
      if (isSelector(selector)) {
        const locals = localsOf(selector);
        for (const result of returnsOf(selector)) {
          const hit = built(result, locals);
          if (hit) report(hit, "the selector's result");
        }
      } else if (selector && calls(selector, SHALLOW_HOOKS) && isSelector(selector.arguments[0])) {
        const inner = selector.arguments[0];
        for (const result of returnsOf(inner)) {
          for (const element of elementsOf(result)) {
            const hit = built(element.of, element.in ? localsOf(element.in) : localsOf(inner));
            if (hit) report(hit, "useShallow compares one level deep, and the element");
          }
        }
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  return found;
}

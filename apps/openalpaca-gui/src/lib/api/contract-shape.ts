/**
 * The API contract's check on this side (API-CONTRACT): a sample the
 * daemon's route tests wrote into `./contract/` (`apps/openalpacad/src/
 * contract/`) is parsed against the TypeScript type that mirrors it.
 *
 * Types are gone at run time, so each type the check covers has a
 * descriptor — a {@link Shape} — that `tsc` ties to it: {@link Presence}
 * makes its `keys` name every key of the type, each `"required"` or
 * `"optional"` exactly as the type says, and its `nested` name a descriptor
 * for every key whose value is an object or an array of objects. A key added
 * to, removed from or renamed in the type fails `bun run check` until the
 * descriptor follows; a key the daemon removes or renames fails vitest,
 * naming the sample and the type. A key the sample carries and the type
 * does not is an addition, and passes: the GUI reads what it knows.
 *
 * An optional key is held as a required one is (CONTRACT-SAMPLES-2; *Rev
 * 107*'s renamed-optional-key item, (b)): the daemon's producers fill every
 * optional value their fixtures can, so a sample lacking an optional key is
 * a rename or a removal — unless its descriptor marks it `"may_omit"`, a key
 * the daemon leaves out of this answer (`skip_serializing_if`, or an
 * outcome the sample is not), each mark with its reason beside it.
 */

/** An array's element, else the type itself. */
type Elem<V> = V extends readonly (infer E)[] ? E : V;

/** What a field's descriptor describes: its object, or its array's. */
export type Inner<V> = NonNullable<Elem<NonNullable<V>>>;

/**
 * Every key of `T`, as the type has it: required, or optional — `"optional"`
 * when the sample must carry it, `"may_omit"` when the daemon may leave it
 * out of the sample's answer.
 */
export type Presence<T> = {
  readonly [K in keyof T]-?: {} extends Pick<T, K>
    ? "optional" | "may_omit"
    : "required";
};

/**
 * The keys of `T` whose value is an object, or an array of objects; not one
 * the type leaves `unknown`, or an array of `unknown`.
 */
type ObjectKeys<T> = {
  [K in keyof T]-?: unknown extends T[K]
    ? never
    : unknown extends Elem<NonNullable<T[K]>>
      ? never
      : Inner<T[K]> extends object
        ? K
        : never;
}[keyof T];

/**
 * A type's descriptor: an object's keys and its nested descriptors; a map's
 * value (an object keyed by data, which the sample writes as one `"*"` key);
 * or `"opaque"`, a value whose shape the type leaves open.
 */
export type Node<T> =
  Shape<T> | { readonly mapOf: Node<Inner<T[keyof T]>> } | "opaque";

/**
 * The nested descriptors `T` needs: one per {@link ObjectKeys}, and none at
 * all where it has none — `Record<string, never>` rather than `{}`, which
 * would let any key through.
 */
type Nested<T> = [ObjectKeys<T>] extends [never]
  ? Readonly<Record<string, never>>
  : { readonly [K in ObjectKeys<T>]-?: Node<Inner<T[K]>> };

export interface Shape<T> {
  readonly keys: Keys<T>;
  readonly nested: Nested<T>;
}

/**
 * Its keys tie a descriptor to `T` only where `T` names them: a type with
 * an index signature (a map keyed by data) has none to tie, so it takes
 * {@link mapOf} or `"opaque"`, and `shape` refuses it.
 */
type Keys<T> = string extends keyof T ? never : Presence<T>;

/** A descriptor for `T`; `tsc` holds both arguments to `T` exactly. */
export function shape<T>(keys: Keys<T>, nested: Shape<T>["nested"]): Shape<T> {
  return { keys, nested };
}

/** A map's descriptor: every value checked against `value`. */
export function mapOf<T>(value: Node<Inner<T[keyof T]>>): Node<T> {
  return { mapOf: value };
}

function join(path: string, key: string): string {
  return path === "" ? key : `${path}.${key}`;
}

/**
 * What in `value` the descriptor cannot read, one line each, empty when the
 * sample parses: a key the type requires, or an optional key not marked
 * `"may_omit"`, that the sample lacks, or an object the type expects where
 * the sample has something else.
 */
export function check<T>(value: unknown, node: Node<T>, path = ""): string[] {
  const out: string[] = [];
  walk(value, node as Node<unknown>, path, out);
  return out;
}

function walk(
  value: unknown,
  node: Node<unknown>,
  path: string,
  out: string[],
): void {
  if (value === null || value === undefined || node === "opaque") return;
  if (Array.isArray(value)) {
    for (const item of value) walk(item, node, `${path}[]`, out);
    return;
  }
  if (typeof value !== "object") {
    out.push(
      `${path || "the body"}: an object in the type, a ${typeof value} in the sample`,
    );
    return;
  }
  const fields = value as Record<string, unknown>;
  if ("mapOf" in node) {
    for (const [key, item] of Object.entries(fields)) {
      walk(item, node.mapOf as Node<unknown>, join(path, key), out);
    }
    return;
  }
  const keys = node.keys as Record<
    string,
    "required" | "optional" | "may_omit"
  >;
  for (const [key, presence] of Object.entries(keys)) {
    if (key in fields || presence === "may_omit") continue;
    out.push(
      presence === "required"
        ? `${join(path, key)}: required by the type, absent from the sample`
        : `${join(path, key)}: optional in the type, absent from the sample ` +
            `(renamed or removed, or a key the daemon may omit: mark it "may_omit")`,
    );
  }
  const nested = node.nested as Record<string, Node<unknown>>;
  for (const [key, child] of Object.entries(nested)) {
    if (key in fields) walk(fields[key], child, join(path, key), out);
  }
}

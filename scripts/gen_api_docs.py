#!/usr/bin/env python3
"""Generate API documentation under docs/api from repository source-of-truth.

Usage:
  python3 scripts/gen_api_docs.py
  python3 scripts/gen_api_docs.py --check
  python3 scripts/tests/test_gen_api_docs.py   # parser regression tests

Parsing rule: every scan for a Rust delimiter (brace, paren, top-level comma)
runs over a comment- and literal-masked copy of the source produced by
`mask_rust_noncode`. Prose is not code — an apostrophe in a doc comment is not
a char literal, a `//` inside a string literal is not a comment, and a comma in
a doc comment is not a variant separator.
"""

from __future__ import annotations

import argparse
import dataclasses
import pathlib
import re
import sys
from collections import defaultdict
from typing import Iterable


ROOT = pathlib.Path(__file__).resolve().parents[1]
DOCS_API = ROOT / "docs" / "api"
ROUTER_FILE = ROOT / "apps" / "openalpacad" / "src" / "router.rs"
ROUTES_DIR = ROOT / "apps" / "openalpacad" / "src" / "routes"
CLI_MAIN = ROOT / "apps" / "openalpaca" / "src" / "main.rs"
CLI_COMMANDS_DIR = ROOT / "apps" / "openalpaca" / "src" / "commands"
GUI_API_DIR = ROOT / "apps" / "openalpaca-gui" / "src" / "lib" / "api"
GUI_DAEMON_FILE = ROOT / "apps" / "openalpaca-gui" / "src" / "lib" / "events.ts"
GUI_TAURI_FILE = ROOT / "apps" / "openalpaca-gui" / "src-tauri" / "src" / "lib.rs"
CARGO_TOML = ROOT / "Cargo.toml"
MIGRATIONS_DIR = ROOT / "crates" / "openalpaca_storage" / "src" / "migrations"
MIGRATIONS_MOD = MIGRATIONS_DIR / "mod.rs"

METHOD_ORDER = {"GET": 0, "POST": 1, "PUT": 2, "DELETE": 3, "PATCH": 4}

RAW_STRING_PREFIX = re.compile(r'b?r(#*)"')


def read_text(path: pathlib.Path) -> str:
    return path.read_text(encoding="utf-8")


def mask_rust_noncode(text: str, *, mask_literals: bool = True) -> str:
    """Return a copy of `text` with everything that is not code blanked out.

    Comments (`//`, `///`, `//!`, `/* */` including nested ones) lose every
    character, markers included; unless `mask_literals=False`, so do the insides
    of string, raw-string and char literals (the delimiters stay). Newlines are
    preserved and nothing changes length, so offsets, line numbers and slices
    taken from the masked copy line up with the original.

    Delimiter scanning must always run over this copy. A lone `'` is a lifetime
    or a label (`&'a str`, `'outer:`), not the start of a literal, so quote
    pairing is decided here once rather than re-guessed by each scanner.
    """
    out = list(text)
    n = len(text)
    i = 0

    def blank(start: int, end: int) -> None:
        for k in range(max(start, 0), min(end, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        ch = text[i]

        # line comment
        if ch == "/" and i + 1 < n and text[i + 1] == "/":
            end = text.find("\n", i)
            end = n if end == -1 else end
            blank(i, end)
            i = end
            continue

        # block comment (Rust allows nesting)
        if ch == "/" and i + 1 < n and text[i + 1] == "*":
            depth = 1
            j = i + 2
            while j < n and depth > 0:
                if text[j] == "/" and j + 1 < n and text[j + 1] == "*":
                    depth += 1
                    j += 2
                elif text[j] == "*" and j + 1 < n and text[j + 1] == "/":
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
            continue

        # raw string: r"..", r#".."#, br#".."#
        if ch in "rb" and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            m = RAW_STRING_PREFIX.match(text, i)
            if m:
                terminator = '"' + m.group(1)
                close = text.find(terminator, m.end())
                if close == -1:
                    content_end, end = n, n
                else:
                    content_end, end = close, close + len(terminator)
                if mask_literals:
                    blank(m.end(), content_end)
                i = end
                continue

        # string literal (also covers the b".." byte-string prefix)
        if ch == '"':
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    break
                j += 1
            if mask_literals:
                blank(i + 1, j)
            i = min(j + 1, n)
            continue

        if ch == "'":
            # escaped char literal: '\n', '\'', '\u{1f600}'
            if i + 1 < n and text[i + 1] == "\\":
                j = i + 3
                while j < n and text[j] != "'":
                    j += 1
                if mask_literals:
                    blank(i + 1, j)
                i = min(j + 1, n)
                continue
            # plain char literal: 'a', '"', '{'
            if i + 2 < n and text[i + 2] == "'":
                if mask_literals:
                    blank(i + 1, i + 2)
                i += 3
                continue
            # lifetime or loop label — carries no delimiter meaning
            i += 1
            continue

        i += 1

    return "".join(out)


def rel(path: pathlib.Path) -> str:
    return path.relative_to(ROOT).as_posix()


def ensure_parent(path: pathlib.Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)


def split_top_level(value: str, sep: str = ",", *, quotes: bool = True) -> list[str]:
    """Split `value` on `sep` at bracket depth zero.

    `quotes=True` tracks quoting inline, which is what the SQL column lists need
    (`DEFAULT 'queued'` must not split). Pass `quotes=False` for Rust code that
    `mask_rust_noncode` already masked: there a lone `'` is a lifetime tick, and
    treating it as an opening quote swallows the rest of the body.
    """
    out: list[str] = []
    cur: list[str] = []
    depth = 0
    in_single = False
    in_double = False
    escape = False

    for ch in value:
        if not quotes:
            if ch in "([{<":
                depth += 1
            elif ch in ")]}>":
                depth = max(0, depth - 1)
            elif ch == sep and depth == 0:
                out.append("".join(cur).strip())
                cur = []
                continue
            cur.append(ch)
            continue

        if escape:
            cur.append(ch)
            escape = False
            continue

        if ch == "\\":
            cur.append(ch)
            escape = True
            continue

        if ch == "'" and not in_double:
            in_single = not in_single
            cur.append(ch)
            continue
        if ch == '"' and not in_single:
            in_double = not in_double
            cur.append(ch)
            continue

        if in_single or in_double:
            cur.append(ch)
            continue

        if ch in "([{<":
            depth += 1
            cur.append(ch)
            continue
        if ch in ")]}>":
            depth = max(0, depth - 1)
            cur.append(ch)
            continue

        if ch == sep and depth == 0:
            out.append("".join(cur).strip())
            cur = []
            continue

        cur.append(ch)

    tail = "".join(cur).strip()
    if tail:
        out.append(tail)
    return out


def extract_brace_block(text: str, open_brace_index: int, scan: str | None = None) -> tuple[str, int]:
    """Return `(body, close_index)` for the brace block opening at the index.

    Braces are counted in `scan`, which must be a masked copy of `text` (same
    length, see `mask_rust_noncode`); the body is sliced out of `text`. Pass the
    masked copy as both to get a masked body — what every identifier-only parse
    below wants, since a doc comment can hold any delimiter at all.
    """
    code = scan if scan is not None else mask_rust_noncode(text)
    depth = 0
    i = open_brace_index

    while i < len(code):
        ch = code[i]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return text[open_brace_index + 1 : i], i
        i += 1

    raise ValueError("Unclosed brace block")


def extract_wrapped_types(params: str, wrapper: str) -> list[str]:
    out: list[str] = []
    needle = f"{wrapper}<"
    i = 0
    while True:
        pos = params.find(needle, i)
        if pos == -1:
            break
        j = pos + len(needle)
        start = j
        depth = 1
        while j < len(params) and depth > 0:
            ch = params[j]
            if ch == "<":
                depth += 1
            elif ch == ">":
                depth -= 1
            j += 1
        if depth == 0:
            out.append(params[start : j - 1].strip())
        i = j
    return out


@dataclasses.dataclass
class Endpoint:
    method: str
    path: str
    auth: str
    handler: str
    module: str
    source: pathlib.Path
    json_type: str | None = None
    query_type: str | None = None


@dataclasses.dataclass
class HandlerMeta:
    module: str
    source: pathlib.Path
    json_types: list[str]
    query_types: list[str]


@dataclasses.dataclass
class TypeDef:
    name: str
    module: str
    source: pathlib.Path
    kind: str
    fields: list[tuple[str, str]]

    @property
    def qualified(self) -> str:
        return f"{self.module}::{self.name}"


@dataclasses.dataclass
class CrateModule:
    name: str
    path: pathlib.Path | None
    cfg: str | None


@dataclasses.dataclass
class CrateDoc:
    name: str
    member_path: pathlib.Path
    lib_path: pathlib.Path
    overview_lines: list[str]
    modules: list[CrateModule]
    re_exports: list[str]


@dataclasses.dataclass
class MigrationItem:
    version: int
    name: str
    file_name: str
    summary: str


@dataclasses.dataclass
class TableDef:
    name: str
    source_file: str
    create_kind: str  # table | virtual table
    columns: list[str]


@dataclasses.dataclass
class IndexDef:
    name: str
    table: str
    expr: str
    source_file: str
    unique: bool


@dataclasses.dataclass
class TriggerDef:
    name: str
    source_file: str


def parse_route_modules() -> tuple[dict[str, HandlerMeta], dict[str, TypeDef]]:
    handler_meta: dict[str, HandlerMeta] = {}
    type_defs: dict[str, TypeDef] = {}

    for path in sorted(ROUTES_DIR.glob("*.rs")):
        if path.name == "mod.rs":
            continue
        text = read_text(path)
        code = mask_rust_noncode(text)
        module = path.stem

        for m in re.finditer(r"pub\s+async\s+fn\s+([A-Za-z0-9_]+)\s*\((.*?)\)\s*->", code, re.S):
            fn_name = m.group(1)
            params = m.group(2)
            json_types = extract_wrapped_types(params, "Json")
            query_types = extract_wrapped_types(params, "Query")
            handler_meta[fn_name] = HandlerMeta(
                module=module,
                source=path,
                json_types=json_types,
                query_types=query_types,
            )

        type_defs.update(parse_type_defs(text, path, module))

    # router-local handlers
    router_text = mask_rust_noncode(read_text(ROUTER_FILE))
    for m in re.finditer(r"async\s+fn\s+([A-Za-z0-9_]+)\s*\((.*?)\)\s*->", router_text, re.S):
        fn_name = m.group(1)
        params = m.group(2)
        json_types = extract_wrapped_types(params, "Json")
        query_types = extract_wrapped_types(params, "Query")
        handler_meta[fn_name] = HandlerMeta(
            module="router",
            source=ROUTER_FILE,
            json_types=json_types,
            query_types=query_types,
        )

    return handler_meta, type_defs


def parse_type_defs_from_file(path: pathlib.Path, module: str) -> dict[str, TypeDef]:
    return parse_type_defs(read_text(path), path, module)


def parse_type_defs(text: str, path: pathlib.Path, module: str) -> dict[str, TypeDef]:
    """Public struct/enum definitions in one file, fields and variants included.

    Everything here reads the masked copy: a declaration quoted inside a doc
    comment is not a declaration, and an apostrophe in one ("the agent's id")
    used to open a phantom char literal that ate the closing brace — dropping
    the struct from the generated docs with no error at all.
    """
    code = mask_rust_noncode(text)
    out: dict[str, TypeDef] = {}

    for m in re.finditer(r"pub\s+(struct|enum)\s+([A-Za-z0-9_]+)", code):
        kind = m.group(1)
        name = m.group(2)

        brace = code.find("{", m.end())
        if brace == -1:
            continue

        # ensure there is no ';' before '{' (tuple/unit structs are skipped)
        semi = code.find(";", m.end(), brace)
        if semi != -1:
            continue

        try:
            body, _ = extract_brace_block(code, brace, scan=code)
        except ValueError:
            continue

        fields: list[tuple[str, str]] = []
        if kind == "struct":
            for line in body.splitlines():
                s = line.strip()
                if not s or s.startswith("#") or s.startswith("//"):
                    continue
                if not s.startswith("pub "):
                    continue
                s = s[4:]
                if ":" not in s:
                    continue
                left, right = s.split(":", 1)
                field_name = left.strip()
                field_type = right.strip()
                if "//" in field_type:
                    field_type = field_type.split("//", 1)[0].strip()
                field_type = field_type.rstrip(",").strip()
                fields.append((field_name, field_type))
        else:
            for line in body.splitlines():
                s = line.strip()
                if not s or s.startswith("#") or s.startswith("//"):
                    continue
                s = s.rstrip(",")
                variant = s.split("(", 1)[0].split("{", 1)[0].strip()
                if not variant or " " in variant:
                    continue
                fields.append((variant, "variant"))

        td = TypeDef(name=name, module=module, source=path, kind=kind, fields=fields)
        out[td.qualified] = td

    return out


def parse_router_endpoints(handler_meta: dict[str, HandlerMeta]) -> list[Endpoint]:
    text = read_text(ROUTER_FILE)
    route_calls = extract_route_calls(text)

    endpoints: list[Endpoint] = []
    for args in route_calls:
        pm = re.search(r'"([^"]+)"', args)
        if not pm:
            continue
        path = pm.group(1)

        mh = re.findall(
            r"\b(get|post|put|delete|patch)\s*\(\s*(?:crate::routes::)?([A-Za-z0-9_]+)\s*\)",
            args,
        )
        for method_lc, handler in mh:
            method = method_lc.upper()
            auth = endpoint_auth(path)
            hm = handler_meta.get(handler)
            module = hm.module if hm else "unknown"
            source = hm.source if hm else ROUTER_FILE
            json_type = hm.json_types[0] if hm and hm.json_types else None
            query_type = hm.query_types[0] if hm and hm.query_types else None
            endpoints.append(
                Endpoint(
                    method=method,
                    path=path,
                    auth=auth,
                    handler=handler,
                    module=module,
                    source=source,
                    json_type=json_type,
                    query_type=query_type,
                )
            )

    endpoints.sort(key=lambda e: (e.path, METHOD_ORDER.get(e.method, 99), e.handler))
    return endpoints


def extract_route_calls(text: str) -> list[str]:
    """The argument text of every `.route(...)` call, parens balanced on the
    masked copy so a comment inside the chain cannot unbalance them. The slices
    come from the original text, because the route path is a string literal."""
    code = mask_rust_noncode(text)
    out: list[str] = []
    needle = ".route("
    i = 0
    while True:
        start = code.find(needle, i)
        if start == -1:
            break
        j = start + len(needle)
        depth = 1
        while j < len(code) and depth > 0:
            ch = code[j]
            if ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
            j += 1
        out.append(text[start + len(needle) : j - 1])
        i = j
    return out


CONTENT_ROUTES = {
    "/v1/files/{id}/content",
    "/v1/artifacts/{id}/content",
    "/v1/artifacts/{id}/versions/{n}/content",
}


def endpoint_auth(path: str) -> str:
    if path in {"/", "/v1/health"}:
        return "none"
    if path in {"/v1/events", "/v1/chat/stream/{stream_id}"}:
        return "query_token"
    if path in CONTENT_ROUTES:
        return "bearer_or_query_token"
    return "bearer"


def qualify_route_type(type_name: str | None, module: str, type_defs: dict[str, TypeDef]) -> str | None:
    if not type_name:
        return None
    t = type_name.strip()
    if "::" in t or "<" in t or "," in t:
        return t
    q = f"{module}::{t}"
    if q in type_defs:
        return q
    return t


def parse_workspace_crates() -> list[pathlib.Path]:
    text = read_text(CARGO_TOML)
    mm = re.search(r"members\s*=\s*\[(.*?)\]", text, re.S)
    if not mm:
        return []
    members = re.findall(r'"([^"]+)"', mm.group(1))
    out: list[pathlib.Path] = []
    for member in members:
        if member.startswith("crates/"):
            out.append(ROOT / member)
    return sorted(out)


def parse_crate_docs() -> list[CrateDoc]:
    out: list[CrateDoc] = []
    for crate_path in parse_workspace_crates():
        lib_path = crate_path / "src" / "lib.rs"
        if not lib_path.exists():
            continue
        text = read_text(lib_path)
        overview_lines = parse_crate_overview_lines(text)
        modules = parse_crate_modules(text, lib_path.parent)
        re_exports = parse_crate_reexports(text)
        out.append(
            CrateDoc(
                name=crate_path.name,
                member_path=crate_path,
                lib_path=lib_path,
                overview_lines=overview_lines,
                modules=modules,
                re_exports=re_exports,
            )
        )
    return out


def parse_crate_overview_lines(text: str) -> list[str]:
    lines = text.splitlines()
    out: list[str] = []
    for line in lines:
        s = line.strip()
        if s.startswith("//!"):
            content = s[3:].strip()
            if content:
                out.append(content)
            continue
        if s == "":
            if out:
                break
            continue
        if out:
            break
        if s.startswith("pub mod") or s.startswith("mod"):
            break
    return out


def parse_crate_modules(text: str, src_dir: pathlib.Path) -> list[CrateModule]:
    modules: list[CrateModule] = []
    pending_cfg: str | None = None
    # comments masked (a commented-out `pub mod` is not a module); literals kept,
    # because the `#[cfg(feature = "…")]` string is rendered as-is.
    for line in mask_rust_noncode(text, mask_literals=False).splitlines():
        s = line.strip()
        if s.startswith("#[cfg"):
            pending_cfg = s
            continue
        m = re.match(r"pub\s+mod\s+([A-Za-z0-9_]+)\s*;", s)
        if not m:
            continue
        name = m.group(1)
        path_rs = src_dir / f"{name}.rs"
        path_mod = src_dir / name / "mod.rs"
        if path_rs.exists():
            path = path_rs
        elif path_mod.exists():
            path = path_mod
        else:
            path = None
        modules.append(CrateModule(name=name, path=path, cfg=pending_cfg))
        pending_cfg = None
    return modules


def parse_crate_reexports(text: str) -> list[str]:
    out: list[str] = []
    for m in re.finditer(r"pub\s+use\s+[^;]+;", mask_rust_noncode(text), re.S):
        item = " ".join(m.group(0).split())
        out.append(item)
    return out


def parse_cli_sources() -> dict[str, object]:
    main_text = read_text(CLI_MAIN)
    top_commands = parse_cli_top_commands(main_text)

    module_docs: dict[str, dict[str, object]] = {}
    for path in sorted(CLI_COMMANDS_DIR.glob("*.rs")):
        text = read_text(path)
        module_name = path.stem
        if module_name == "mod":
            continue

        enums = parse_subcommand_enums(text)
        flags = parse_clap_flags(text)

        module_docs[module_name] = {
            "path": path,
            "enums": enums,
            "flags": flags,
        }

    return {
        "top_commands": top_commands,
        "modules": module_docs,
    }


def parse_cli_top_commands(text: str) -> list[tuple[str, str, str]]:
    code = mask_rust_noncode(text)
    m = re.search(r"enum\s+Commands\s*\{", code)
    if not m:
        return []
    try:
        # body from `text`, not from `code`: the purpose column is doc-comment prose.
        body, _ = extract_brace_block(text, m.end() - 1, scan=code)
    except ValueError:
        return []
    out: list[tuple[str, str, str]] = []
    pattern = re.compile(
        r"///\s*(.+?)\n\s*([A-Za-z0-9_]+)\s*\(\s*commands::([a-z0-9_]+)::",
        re.S,
    )
    for mm in pattern.finditer(body):
        desc = " ".join(mm.group(1).split())
        variant = mm.group(2)
        module = mm.group(3)
        cmd = camel_to_kebab(variant)
        out.append((cmd, desc, module))
    return out


def parse_subcommand_enums(text: str) -> list[dict[str, object]]:
    code = mask_rust_noncode(text)
    enums: list[dict[str, object]] = []
    for m in re.finditer(r"#\[derive\(Subcommand\)\]\s*pub\s+enum\s+([A-Za-z0-9_]+)\s*\{", code, re.S):
        enum_name = m.group(1)
        brace = code.find("{", m.end() - 1)
        if brace == -1:
            continue
        try:
            # masked body: variants and field names are code, the help text is not.
            body, _ = extract_brace_block(code, brace, scan=code)
        except ValueError:
            continue
        variants = parse_enum_variants(body)
        enums.append({"name": enum_name, "variants": variants})
    return enums


def parse_enum_variants(body: str) -> list[dict[str, object]]:
    """Variant names and struct-field names of one enum body.

    `body` comes masked, so the split sees only real separators: a comma in a
    variant's clap help text used to start a new entry and invent a variant out
    of the words after it, and a plain `//` line used to shadow the variant that
    followed it.
    """
    entries = split_top_level(body, quotes=False)
    out: list[dict[str, object]] = []
    for entry in entries:
        if not entry:
            continue
        cleaned_lines = []
        for line in entry.splitlines():
            s = line.strip()
            if not s or s.startswith("//") or s.startswith("#["):
                continue
            cleaned_lines.append(s)
        if not cleaned_lines:
            continue
        merged = " ".join(cleaned_lines)
        vm = re.match(r"([A-Za-z0-9_]+)", merged)
        if not vm:
            continue
        variant = vm.group(1)

        field_names: list[str] = []
        if "{" in merged and "}" in merged:
            block = merged.split("{", 1)[1].rsplit("}", 1)[0]
            for field in split_top_level(block, quotes=False):
                fm = re.match(r"([A-Za-z0-9_]+)\s*:", field.strip())
                if fm:
                    field_names.append(fm.group(1))

        out.append(
            {
                "name": variant,
                "command": camel_to_kebab(variant),
                "fields": field_names,
            }
        )
    return out


def parse_clap_flags(text: str) -> list[str]:
    flags: set[str] = set()
    # literals stay: the flag name is `long = "…"`, the short is a char literal.
    code = mask_rust_noncode(text, mask_literals=False)
    for m in re.finditer(r"#\[arg\((.*?)\)\]\s*([A-Za-z0-9_]+)\s*:", code, re.S):
        attrs = m.group(1)
        field = m.group(2)

        long_explicit = re.search(r'long\s*=\s*"([A-Za-z0-9_-]+)"', attrs)
        if long_explicit:
            flags.add(f"--{long_explicit.group(1)}")
        elif re.search(r"\blong\b", attrs):
            flags.add(f"--{field.replace('_', '-')}")

        short_explicit = re.search(r"short\s*=\s*'([A-Za-z0-9])'", attrs)
        if short_explicit:
            flags.add(f"-{short_explicit.group(1)}")

    return sorted(flags)


GUI_DOC_BLOCK = re.compile(r"/\*\*.*?\*/", re.S)
GUI_DOC_ENDPOINT = re.compile(r"\b(GET|POST|PUT|DELETE|PATCH)\s+(/v1/[^\s`*]+)")
GUI_EXPORTED_FN = re.compile(r"export\s+async\s+function\s+([A-Za-z0-9_]+)")


def gui_module_header_doc(text: str) -> str:
    """The module's own header doc block — the first one standing above the
    imports. A doc block that follows them belongs to a type or a function."""
    block = GUI_DOC_BLOCK.search(text)
    if not block:
        return ""
    stmt = re.search(r"^(?:import|export)\b", text, re.M)
    first_stmt = stmt.start() if stmt else len(text)
    return block.group(0) if block.end() <= first_stmt else ""


def gui_preceding_doc(text: str, pos: int) -> str:
    """The doc block immediately above `pos`, or an empty string."""
    head = text[:pos].rstrip()
    if not head.endswith("*/"):
        return ""
    start = head.rfind("/**")
    return head[start:] if start != -1 else ""


def gui_first_endpoint(doc: str) -> tuple[str, str] | None:
    m = GUI_DOC_ENDPOINT.search(doc)
    if not m:
        return None
    path = m.group(2).split("?", 1)[0].split("#", 1)[0].rstrip("`.,;:)\"'")
    if not path.startswith("/v1/"):
        return None
    return m.group(1), path


def parse_gui_endpoints(text: str) -> list[tuple[str, str]]:
    """The daemon endpoint each exported wrapper of one GUI api module calls.

    These modules document the route in the doc comment above the wrapper —
    ``/** `GET /v1/tools` — the tool catalog. */`` — so that block is where the
    method and path are read from, falling back to the module header when the
    wrapper's own block names none (tools.ts). Earlier revisions matched the
    method only immediately after `/**`, which the backtick convention broke:
    the table rendered empty for every module. A path keeps only what the router
    registers — the query string of an example (`?path=`) is dropped — and a
    wrapper built on another wrapper (telemetry's `getRunEventLog`) contributes
    no row, because it calls no route of its own.
    """
    header = gui_module_header_doc(text)
    out: list[tuple[str, str]] = []
    seen: set[tuple[str, str]] = set()
    for m in GUI_EXPORTED_FN.finditer(text):
        hit = gui_first_endpoint(gui_preceding_doc(text, m.start())) or gui_first_endpoint(header)
        if hit and hit not in seen:
            seen.add(hit)
            out.append(hit)
    return out


def parse_gui_sources() -> dict[str, object]:
    modules: list[dict[str, object]] = []
    source_paths: set[pathlib.Path] = set()

    for path in sorted(GUI_API_DIR.glob("*.ts")):
        text = read_text(path)
        funcs = re.findall(r"export\s+async\s+function\s+([A-Za-z0-9_]+)", text)
        endpoints = parse_gui_endpoints(text)
        modules.append(
            {
                "name": path.name,
                "path": path,
                "functions": sorted(funcs),
                "endpoints": endpoints,
            }
        )
        source_paths.add(path)

    daemon_text = read_text(GUI_DAEMON_FILE)
    ws_events = sorted(set(re.findall(r'type:\s*"([a-z0-9_]+)"', daemon_text)))

    tauri_text = read_text(GUI_TAURI_FILE)
    tauri_commands = re.findall(
        r"#\[tauri::command\]\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)",
        tauri_text,
        re.S,
    )

    return {
        "modules": modules,
        "ws_events": ws_events,
        "tauri_commands": tauri_commands,
        "daemon_file": GUI_DAEMON_FILE,
        "tauri_file": GUI_TAURI_FILE,
    }


def parse_migrations() -> list[MigrationItem]:
    # literals stay: the name and the SQL file are string literals in the registry.
    text = mask_rust_noncode(read_text(MIGRATIONS_MOD), mask_literals=False)
    items: list[MigrationItem] = []
    for block in re.findall(r"Migration\s*\{(.*?)\}", text, re.S):
        vm = re.search(r"version:\s*(\d+)", block)
        nm = re.search(r'name:\s*"([^"]+)"', block)
        fm = re.search(r'include_str!\("([^"]+)"\)', block)
        if not (vm and nm and fm):
            continue
        version = int(vm.group(1))
        name = nm.group(1)
        file_name = fm.group(1)
        summary = migration_summary(MIGRATIONS_DIR / file_name)
        items.append(
            MigrationItem(
                version=version,
                name=name,
                file_name=file_name,
                summary=summary,
            )
        )

    items.sort(key=lambda i: i.version)
    return items


def migration_summary(path: pathlib.Path) -> str:
    for line in read_text(path).splitlines():
        s = line.strip()
        if s.startswith("--"):
            text = s[2:].strip()
            if text:
                return text
    return "(no summary comment)"


def strip_sql_comments(sql: str) -> str:
    out_lines = []
    for line in sql.splitlines():
        if "--" in line:
            idx = line.find("--")
            line = line[:idx]
        out_lines.append(line)
    return "\n".join(out_lines)


def split_sql_statements(sql: str) -> list[str]:
    stmts: list[str] = []
    cur: list[str] = []
    in_single = False
    in_double = False
    escape = False
    depth = 0

    for ch in sql:
        if escape:
            cur.append(ch)
            escape = False
            continue
        if ch == "\\":
            cur.append(ch)
            escape = True
            continue
        if ch == "'" and not in_double:
            in_single = not in_single
            cur.append(ch)
            continue
        if ch == '"' and not in_single:
            in_double = not in_double
            cur.append(ch)
            continue
        if in_single or in_double:
            cur.append(ch)
            continue

        if ch == "(":
            depth += 1
        elif ch == ")":
            depth = max(0, depth - 1)

        if ch == ";" and depth == 0:
            statement = "".join(cur).strip()
            if statement:
                stmts.append(statement)
            cur = []
            continue

        cur.append(ch)

    tail = "".join(cur).strip()
    if tail:
        stmts.append(tail)
    return stmts


def parse_schema_state(migrations: list[MigrationItem]) -> tuple[dict[str, TableDef], dict[str, IndexDef], dict[str, TriggerDef]]:
    tables: dict[str, TableDef] = {}
    indexes: dict[str, IndexDef] = {}
    triggers: dict[str, TriggerDef] = {}

    for mig in migrations:
        sql_path = MIGRATIONS_DIR / mig.file_name
        sql = strip_sql_comments(read_text(sql_path))
        for stmt in split_sql_statements(sql):
            s = " ".join(stmt.split())
            s_upper = s.upper()

            m = re.match(
                r"CREATE\s+(VIRTUAL\s+)?TABLE\s+(IF\s+NOT\s+EXISTS\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\((.*)\)$",
                s,
                re.I | re.S,
            )
            if m:
                is_virtual = bool(m.group(1))
                table_name = m.group(3)
                cols_body = m.group(4).strip()
                cols = [c.strip() for c in split_top_level(cols_body) if c.strip()]
                tables[table_name] = TableDef(
                    name=table_name,
                    source_file=mig.file_name,
                    create_kind="virtual table" if is_virtual else "table",
                    columns=cols,
                )
                continue

            m = re.match(r"DROP\s+TABLE\s+IF\s+EXISTS\s+([A-Za-z_][A-Za-z0-9_]*)$", s, re.I)
            if m:
                tables.pop(m.group(1), None)
                continue

            m = re.match(
                r"ALTER\s+TABLE\s+([A-Za-z_][A-Za-z0-9_]*)\s+RENAME\s+TO\s+([A-Za-z_][A-Za-z0-9_]*)$",
                s,
                re.I,
            )
            if m:
                old = m.group(1)
                new = m.group(2)
                if old in tables:
                    td = tables.pop(old)
                    td.name = new
                    td.source_file = mig.file_name
                    tables[new] = td
                continue

            m = re.match(
                r"ALTER\s+TABLE\s+([A-Za-z_][A-Za-z0-9_]*)\s+ADD\s+COLUMN\s+(.+)$",
                s,
                re.I,
            )
            if m:
                table_name = m.group(1)
                col_def = m.group(2).strip()
                if table_name in tables:
                    tables[table_name].columns.append(col_def)
                    tables[table_name].source_file = mig.file_name
                continue

            m = re.match(
                r"CREATE\s+(UNIQUE\s+)?INDEX\s+(IF\s+NOT\s+EXISTS\s+)?([A-Za-z_][A-Za-z0-9_]*)\s+ON\s+([A-Za-z_][A-Za-z0-9_]*)\s*\((.+)\)$",
                s,
                re.I | re.S,
            )
            if m:
                idx_name = m.group(3)
                table_name = m.group(4)
                expr = m.group(5).strip()
                indexes[idx_name] = IndexDef(
                    name=idx_name,
                    table=table_name,
                    expr=expr,
                    source_file=mig.file_name,
                    unique=bool(m.group(1)),
                )
                continue

            m = re.match(r"DROP\s+INDEX\s+IF\s+EXISTS\s+([A-Za-z_][A-Za-z0-9_]*)$", s, re.I)
            if m:
                indexes.pop(m.group(1), None)
                continue

            m = re.match(
                r"CREATE\s+TRIGGER\s+(IF\s+NOT\s+EXISTS\s+)?([A-Za-z_][A-Za-z0-9_]*)\s+",
                s,
                re.I,
            )
            if m:
                trg_name = m.group(2)
                triggers[trg_name] = TriggerDef(name=trg_name, source_file=mig.file_name)
                continue

            m = re.match(r"DROP\s+TRIGGER\s+IF\s+EXISTS\s+([A-Za-z_][A-Za-z0-9_]*)$", s, re.I)
            if m:
                triggers.pop(m.group(1), None)
                continue

            _ = s_upper

    return tables, indexes, triggers


def markdown_table(headers: list[str], rows: list[list[str]]) -> str:
    lines = ["| " + " | ".join(headers) + " |", "|" + "|".join(["---"] * len(headers)) + "|"]
    for row in rows:
        lines.append("| " + " | ".join(row) + " |")
    return "\n".join(lines)


def generate_api_readme(crate_docs: list[CrateDoc]) -> str:
    app_items = [
        ("openalpacad", "apps/openalpacad.md"),
        ("openalpaca (CLI)", "apps/openalpaca.md"),
        ("openalpaca-gui", "apps/openalpaca-gui.md"),
    ]

    crate_items = [(c.name, f"crates/{c.name}.md") for c in sorted(crate_docs, key=lambda c: c.name)]

    lines = [
        "# API Docs",
        "",
        "> Generated from source by `python3 scripts/gen_api_docs.py`.",
        "> Validate freshness with `python3 scripts/gen_api_docs.py --check`.",
        "",
        "## Apps",
        "",
    ]

    for name, path in app_items:
        lines.append(f"- [{name}]({path})")

    lines.extend(["", "## Crates", ""])
    for name, path in crate_items:
        lines.append(f"- [{name}]({path})")

    lines.extend(
        [
            "",
            "## Database",
            "",
            "- [Schema](database/schema.md)",
            "- [Migrations](database/migrations.md)",
            "",
            "## Validation Guarantees",
            "",
            "- Route parity: every route in `apps/openalpacad/src/router.rs` is rendered in the daemon API table.",
            "- Migration parity: migration files listed in `crates/openalpaca_storage/src/migrations/mod.rs` match generated migration docs.",
            "- Source link integrity: generated source file references are verified to exist.",
        ]
    )

    return "\n".join(lines).rstrip() + "\n"


def generate_openalpacad_doc(
    endpoints: list[Endpoint],
    type_defs: dict[str, TypeDef],
) -> str:
    # Qualify request/query type names against module-local route types.
    for ep in endpoints:
        ep.json_type = qualify_route_type(ep.json_type, ep.module, type_defs)
        ep.query_type = qualify_route_type(ep.query_type, ep.module, type_defs)

    endpoint_rows = []
    for ep in endpoints:
        endpoint_rows.append(
            [
                ep.method,
                f"`{ep.path}`",
                f"`{ep.auth}`",
                f"`{ep.handler}`",
                f"`{ep.json_type}`" if ep.json_type else "-",
                f"`{ep.query_type}`" if ep.query_type else "-",
                f"`{rel(ep.source)}`",
            ]
        )

    request_types: list[str] = []
    for ep in endpoints:
        if ep.json_type:
            request_types.append(ep.json_type)
        if ep.query_type:
            request_types.append(ep.query_type)
    request_types = sorted(set(request_types))

    response_types: list[TypeDef] = []
    response_seen = set()
    endpoint_modules = {ep.module for ep in endpoints}
    for td in type_defs.values():
        if td.module in endpoint_modules and "response" in td.name.lower():
            if td.qualified not in response_seen:
                response_seen.add(td.qualified)
                response_types.append(td)
    response_types.sort(key=lambda x: (x.module, x.name))

    lines = [
        "# openalpacad HTTP API",
        "",
        "> Generated from source by `python3 scripts/gen_api_docs.py`.",
        "",
        "## Overview",
        "",
        "- Router source: `apps/openalpacad/src/router.rs`.",
        f"- Total documented method/path endpoints: {len(endpoints)}.",
        "- Includes public, bearer-protected, WebSocket, and SSE routes.",
        "",
        "## Auth",
        "",
        "- `none`: public endpoints (`/`, `/v1/health`).",
        "- `bearer`: `Authorization: Bearer <token>` from discovery metadata.",
        "- `query_token`: token via query string for streaming (`/v1/events`, `/v1/chat/stream/{stream_id}`).",
        "- `bearer_or_query_token`: content routes validate the token inline and accept either form, so a webview `<img src>`/`<iframe src>` can load bytes (GAP-11).",
        "",
        "## Endpoints",
        "",
        markdown_table(
            ["Method", "Path", "Auth", "Handler", "JSON Body", "Query", "Source"],
            endpoint_rows,
        ),
        "",
        "## Request/Query Types",
        "",
    ]

    if not request_types:
        lines.append("No request/query types were discovered.")
    else:
        for qn in request_types:
            lines.append(f"### `{qn}`")
            td = type_defs.get(qn)
            if td is None:
                lines.append("")
                lines.append("- External or generic type; see handler source.")
                lines.append("")
                continue

            lines.append("")
            lines.append(f"- Kind: `{td.kind}`")
            lines.append(f"- Source: `{rel(td.source)}`")
            if not td.fields:
                lines.append("- No public fields parsed.")
                lines.append("")
                continue
            lines.append("")
            rows = [[f"`{name}`", f"`{ftype}`"] for name, ftype in td.fields]
            lines.append(markdown_table(["Field", "Type"], rows))
            lines.append("")

    lines.extend(["## Response Shapes", ""])
    if not response_types:
        lines.append("No route-local response structs were discovered.")
        lines.append("")
    else:
        for td in response_types:
            lines.append(f"### `{td.qualified}`")
            lines.append("")
            lines.append(f"- Kind: `{td.kind}`")
            lines.append(f"- Source: `{rel(td.source)}`")
            if td.fields:
                lines.append("")
                rows = [[f"`{name}`", f"`{ftype}`"] for name, ftype in td.fields]
                lines.append(markdown_table(["Field", "Type"], rows))
            else:
                lines.append("- No public fields parsed.")
            lines.append("")

    lines.extend(
        [
            "## Streaming",
            "",
            "- WebSocket `GET /v1/events?token=...` sends `openalpaca_api::events::ServerEvent` JSON payloads.",
            "- SSE `GET /v1/chat/stream/{stream_id}?token=...` emits events: `thinking`, `delta`, `done`, `error`.",
            "",
            "## Related Links",
            "",
            "- [CLI API doc](openalpaca.md)",
            "- [GUI API doc](openalpaca-gui.md)",
            "- [Database Schema](../database/schema.md)",
            "",
        ]
    )

    return "\n".join(lines).rstrip() + "\n"


def generate_openalpaca_cli_doc(cli_info: dict[str, object]) -> str:
    top_commands = cli_info["top_commands"]  # type: ignore[index]
    modules = cli_info["modules"]  # type: ignore[index]

    lines = [
        "# openalpaca (CLI)",
        "",
        "> Generated from source by `python3 scripts/gen_api_docs.py`.",
        "",
        "## Overview",
        "",
        "- Entry point: `apps/openalpaca/src/main.rs`.",
        "- Command modules: `apps/openalpaca/src/commands/*.rs`.",
        "- The CLI resolves daemon connection/auth from `discovery.json`.",
        "",
        "## Auth",
        "",
        "- Reads discovery token and sends `Authorization: Bearer <token>` to protected daemon routes.",
        "- Uses query token for chat/event streaming endpoints where required.",
        "",
        "## Endpoints",
        "",
    ]

    endpoint_rows = []
    for cmd, desc, module in top_commands:
        endpoint_rows.append([
            f"`openalpaca {cmd}`",
            desc,
            f"`apps/openalpaca/src/commands/{module}.rs`",
        ])
    lines.append(markdown_table(["Command", "Purpose", "Source"], endpoint_rows))
    lines.append("")

    lines.append("## Request/Query Types")
    lines.append("")
    lines.append("- CLI argument and subcommand types are defined with `clap` derive structs/enums in command modules.")
    lines.append("")

    lines.append("## Response Shapes")
    lines.append("")
    lines.append("- Output is user-facing table/json text emitted by each command module.")
    lines.append("- Machine-readable outputs are gated by `--format json` where supported.")
    lines.append("")

    lines.append("## Streaming")
    lines.append("")
    lines.append("- `openalpaca daemon tail` consumes daemon event WebSocket stream.")
    lines.append("- `openalpaca chat` streams SSE chat output when interacting with daemon chat routes.")
    lines.append("")

    lines.append("## Related Links")
    lines.append("")
    lines.append("- [Daemon API](openalpacad.md)")
    lines.append("- [GUI API](openalpaca-gui.md)")
    lines.append("")

    lines.append("## Command Source Map")
    lines.append("")

    for module_name in sorted(modules.keys()):
        info = modules[module_name]
        path = info["path"]
        enums = info["enums"]
        flags = info["flags"]

        lines.append(f"### `{module_name}`")
        lines.append("")
        lines.append(f"- Source: `{rel(path)}`")
        if enums:
            for enum_info in enums:
                lines.append(f"- Enum `{enum_info['name']}` variants:")
                for variant in enum_info["variants"]:
                    fields = variant["fields"]
                    if fields:
                        lines.append(
                            f"  - `{variant['command']}` (fields: {', '.join(f'`{f}`' for f in fields)})"
                        )
                    else:
                        lines.append(f"  - `{variant['command']}`")
        else:
            lines.append("- No `Subcommand` enum found in module.")
        if flags:
            lines.append(f"- Parsed flags: {', '.join(f'`{f}`' for f in flags)}")
        else:
            lines.append("- Parsed flags: none")
        lines.append("")

    return "\n".join(lines).rstrip() + "\n"


def generate_openalpaca_gui_doc(gui_info: dict[str, object]) -> str:
    modules = gui_info["modules"]  # type: ignore[index]
    ws_events = gui_info["ws_events"]  # type: ignore[index]
    tauri_commands = gui_info["tauri_commands"]  # type: ignore[index]
    daemon_file = gui_info["daemon_file"]  # type: ignore[index]
    tauri_file = gui_info["tauri_file"]  # type: ignore[index]

    endpoint_rows: list[list[str]] = []
    for mod in modules:
        for method, path in mod["endpoints"]:
            endpoint_rows.append([
                method,
                f"`{path}`",
                f"`{mod['name']}`",
                f"`{rel(mod['path'])}`",
            ])
    endpoint_rows.sort(key=lambda r: (r[1], METHOD_ORDER.get(r[0], 99), r[2]))

    lines = [
        "# openalpaca-gui (Tauri + React)",
        "",
        "> Generated from source by `python3 scripts/gen_api_docs.py`.",
        "",
        "## Overview",
        "",
        "- Frontend API wrappers: `apps/openalpaca-gui/src/lib/api/*.ts`.",
        "- Tauri backend commands: `apps/openalpaca-gui/src-tauri/src/lib.rs`.",
        "- Daemon event stream client: `apps/openalpaca-gui/src/lib/events.ts`.",
        "",
        "## Auth",
        "",
        "- HTTP API calls use `Authorization: Bearer <token>` from discovery connection info.",
        "- WebSocket uses query token: `/v1/events?token=...`.",
        "- SSE chat stream uses query token: `/v1/chat/stream/{stream_id}?token=...`.",
        "",
        "## Endpoints",
        "",
        "- One row per exported wrapper, read from the route named in its doc comment",
        "  (the module header when the wrapper names none). Paths are spelled as the",
        "  client documents them, so a path parameter can differ from the router's",
        "  (`{id}` for `{message_id}`) and a `{kind}` segment can arrive already filled",
        "  in (`/v1/extensions/plugin/{id}`); query strings are dropped.",
        "",
        markdown_table(["Method", "Path", "Module", "Source"], endpoint_rows),
        "",
        "## Request/Query Types",
        "",
        "- Request/query payloads are represented as TypeScript interfaces in `apps/openalpaca-gui/src/lib/types.ts`",
        "  and module-local request types under `apps/openalpaca-gui/src/lib/api/*.ts`.",
        "",
        "## Response Shapes",
        "",
        "- Response interfaces include task/agent/settings/conversation usage models in `apps/openalpaca-gui/src/lib/types.ts`.",
        "",
        "## Streaming",
        "",
        f"- WebSocket client source: `{rel(daemon_file)}`.",
        "- Parsed `ServerEvent` discriminators:",
    ]

    if ws_events:
        lines.append("- " + ", ".join(f"`{name}`" for name in ws_events))
    else:
        lines.append("- (none parsed)")

    lines.extend(
        [
            "",
            "## Related Links",
            "",
            "- [Daemon API](openalpacad.md)",
            "- [CLI API](openalpaca.md)",
            "",
            "## Tauri Commands",
            "",
            f"- Source: `{rel(tauri_file)}`",
            "- Commands: " + ", ".join(f"`{cmd}`" for cmd in sorted(tauri_commands)),
            "",
            "## API Module Map",
            "",
        ]
    )

    for mod in modules:
        lines.append(f"### `{mod['name']}`")
        lines.append("")
        lines.append(f"- Source: `{rel(mod['path'])}`")
        if mod["functions"]:
            lines.append(
                "- Exported functions: " + ", ".join(f"`{fn}`" for fn in sorted(mod["functions"]))
            )
        else:
            lines.append("- Exported functions: none")
        if mod["endpoints"]:
            lines.append(
                "- Endpoints: "
                + ", ".join(f"`{m} {p}`" for m, p in mod["endpoints"])
            )
        else:
            lines.append("- Endpoints: none")
        lines.append("")

    return "\n".join(lines).rstrip() + "\n"


def generate_crate_doc(crate: CrateDoc) -> str:
    lines = [
        f"# `{crate.name}`",
        "",
        "> Generated from source by `python3 scripts/gen_api_docs.py`.",
        "",
        "## Overview",
        "",
        f"- Member path: `{rel(crate.member_path)}`",
        f"- Entry: `{rel(crate.lib_path)}`",
    ]

    if crate.overview_lines:
        lines.append("")
        for line in crate.overview_lines:
            lines.append(f"- {line}")

    lines.extend(["", "## Modules", ""])
    if crate.modules:
        for mod in crate.modules:
            bits = [f"`{mod.name}`"]
            if mod.path is not None:
                bits.append(f"({rel(mod.path)})")
            else:
                bits.append("(path unresolved)")
            line = "- " + " ".join(bits)
            if mod.cfg:
                line += f" `{mod.cfg}`"
            lines.append(line)
    else:
        lines.append("- No `pub mod` declarations found in crate root.")

    lines.extend(["", "## Re-exports", ""])
    if crate.re_exports:
        for item in crate.re_exports:
            lines.append(f"- `{item}`")
    else:
        lines.append("- No root-level `pub use` re-exports found.")

    lines.extend(["", "## Related Links", "", "- [API Index](../README.md)", ""])
    return "\n".join(lines).rstrip() + "\n"


def generate_migrations_doc(migrations: list[MigrationItem]) -> str:
    rows = []
    for m in migrations:
        rows.append(
            [
                str(m.version),
                f"`{m.name}`",
                f"`{m.file_name}`",
                m.summary,
            ]
        )

    lines = [
        "# Database Migrations",
        "",
        "> Generated from migration registry in `crates/openalpaca_storage/src/migrations/mod.rs`.",
        "",
        "## Overview",
        "",
        f"- Total registered migrations: {len(migrations)}",
        f"- Migration SQL directory: `{rel(MIGRATIONS_DIR)}`",
        "",
        "## Files",
        "",
        markdown_table(["Version", "Name", "SQL File", "Summary"], rows),
        "",
    ]
    return "\n".join(lines).rstrip() + "\n"


def generate_schema_doc(
    migrations: list[MigrationItem],
    tables: dict[str, TableDef],
    indexes: dict[str, IndexDef],
    triggers: dict[str, TriggerDef],
) -> str:
    table_items = sorted(tables.values(), key=lambda t: t.name)
    index_items = sorted(indexes.values(), key=lambda i: (i.table, i.name))
    trigger_items = sorted(triggers.values(), key=lambda t: t.name)

    lines = [
        "# Database Schema (SQLite)",
        "",
        "> Generated from migration SQL in `crates/openalpaca_storage/src/migrations/*.sql`.",
        "",
        "## Files",
        "",
        "- DB path resolver: `openalpaca_storage::paths::database_path()`",
        "- Migrations entrypoint: `openalpaca_storage::migrations::MIGRATIONS`",
        f"- Registered migrations: {len(migrations)}",
        "",
        "## Tables",
        "",
    ]

    for td in table_items:
        lines.append(f"### `{td.name}` ({td.create_kind})")
        lines.append("")
        lines.append(f"Source migration: `{td.source_file}`")
        lines.append("")
        lines.append("```sql")
        for col in td.columns:
            lines.append(col)
        lines.append("```")
        lines.append("")

    lines.extend(["## Indexes", ""])
    if index_items:
        rows = [
            [
                f"`{idx.name}`",
                f"`{idx.table}`",
                "`UNIQUE`" if idx.unique else "`INDEX`",
                f"`{idx.expr}`",
                f"`{idx.source_file}`",
            ]
            for idx in index_items
        ]
        lines.append(markdown_table(["Name", "Table", "Kind", "Columns/Expr", "Source"], rows))
    else:
        lines.append("No active indexes parsed.")
    lines.append("")

    lines.extend(["## Triggers", ""])
    if trigger_items:
        rows = [[f"`{trg.name}`", f"`{trg.source_file}`"] for trg in trigger_items]
        lines.append(markdown_table(["Name", "Source"], rows))
    else:
        lines.append("No active triggers parsed.")
    lines.append("")

    return "\n".join(lines).rstrip() + "\n"


def camel_to_kebab(value: str) -> str:
    s1 = re.sub("(.)([A-Z][a-z]+)", r"\1-\2", value)
    s2 = re.sub("([a-z0-9])([A-Z])", r"\1-\2", s1)
    return s2.lower()


def collect_generated_docs() -> dict[pathlib.Path, str]:
    handler_meta, route_type_defs = parse_route_modules()
    endpoints = parse_router_endpoints(handler_meta)

    crate_docs = parse_crate_docs()
    cli_info = parse_cli_sources()
    gui_info = parse_gui_sources()
    migrations = parse_migrations()
    tables, indexes, triggers = parse_schema_state(migrations)

    outputs: dict[pathlib.Path, str] = {}
    outputs[DOCS_API / "README.md"] = generate_api_readme(crate_docs)
    outputs[DOCS_API / "apps" / "openalpacad.md"] = generate_openalpacad_doc(endpoints, route_type_defs)
    outputs[DOCS_API / "apps" / "openalpaca.md"] = generate_openalpaca_cli_doc(cli_info)
    outputs[DOCS_API / "apps" / "openalpaca-gui.md"] = generate_openalpaca_gui_doc(gui_info)

    for crate in crate_docs:
        outputs[DOCS_API / "crates" / f"{crate.name}.md"] = generate_crate_doc(crate)

    outputs[DOCS_API / "database" / "migrations.md"] = generate_migrations_doc(migrations)
    outputs[DOCS_API / "database" / "schema.md"] = generate_schema_doc(migrations, tables, indexes, triggers)

    run_consistency_assertions(outputs, endpoints, migrations, crate_docs, gui_info)
    return outputs


def run_consistency_assertions(
    outputs: dict[pathlib.Path, str],
    endpoints: list[Endpoint],
    migrations: list[MigrationItem],
    crate_docs: list[CrateDoc],
    gui_info: dict[str, object],
) -> None:
    # Endpoint parity invariant (router parse should have at least all method/path rows)
    if not endpoints:
        raise RuntimeError("No daemon endpoints parsed from router")

    # Migration parity invariant
    sql_files = sorted(MIGRATIONS_DIR.glob("[0-9][0-9][0-9]_*.sql"))
    if len(sql_files) != len(migrations):
        raise RuntimeError(
            f"Migration parity check failed: {len(migrations)} registered vs {len(sql_files)} SQL files"
        )

    # Generated source reference integrity
    existing_paths: set[pathlib.Path] = set()
    for ep in endpoints:
        existing_paths.add(ep.source)
    for crate in crate_docs:
        existing_paths.add(crate.lib_path)
        for mod in crate.modules:
            if mod.path is not None:
                existing_paths.add(mod.path)

    for mod in gui_info["modules"]:  # type: ignore[index]
        existing_paths.add(mod["path"])
    existing_paths.add(gui_info["daemon_file"])  # type: ignore[arg-type]
    existing_paths.add(gui_info["tauri_file"])  # type: ignore[arg-type]

    for path in existing_paths:
        if not path.exists():
            raise RuntimeError(f"Source link integrity check failed: missing {rel(path)}")

    # Verify daemon endpoint table row count matches parsed router count.
    daemon_doc = outputs[DOCS_API / "apps" / "openalpacad.md"]
    row_count = len(re.findall(r"^\|\s*(GET|POST|PUT|DELETE|PATCH)\s*\|", daemon_doc, re.M))
    if row_count != len(endpoints):
        raise RuntimeError(
            f"Endpoint table parity failed: table rows={row_count} parsed endpoints={len(endpoints)}"
        )


def apply_outputs(outputs: dict[pathlib.Path, str], check: bool) -> int:
    changed: list[pathlib.Path] = []

    for path, content in sorted(outputs.items(), key=lambda kv: kv[0].as_posix()):
        current = path.read_text(encoding="utf-8") if path.exists() else None
        if current != content:
            changed.append(path)
            if not check:
                ensure_parent(path)
                path.write_text(content, encoding="utf-8")

    if check:
        if changed:
            print("API documentation is stale. Regenerate with:")
            print("  python3 scripts/gen_api_docs.py")
            print("\nChanged files:")
            for p in changed:
                print(f"- {rel(p)}")
            return 1
        print("API documentation is up to date.")
        return 0

    if changed:
        print("Updated documentation files:")
        for p in changed:
            print(f"- {rel(p)}")
    else:
        print("No documentation changes were necessary.")
    return 0


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Generate docs/api markdown from source.")
    parser.add_argument("--check", action="store_true", help="Exit non-zero if docs are stale.")
    args = parser.parse_args(list(argv) if argv is not None else None)

    outputs = collect_generated_docs()
    return apply_outputs(outputs, check=args.check)


if __name__ == "__main__":
    sys.exit(main())

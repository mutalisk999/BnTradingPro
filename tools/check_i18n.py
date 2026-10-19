"""提取 src/ 下 UI 可达的中文字符串字面量，与 i18n 字典键比对。

过滤规则：
- 剔除注释与字符串转义（含 r"..." / r#"..."# raw string）
- 剔除日志宏（tracing 的 info!/warn!/error!/debug!/trace!）、panic!/unreachable!、
  assert 系列、.expect( 语句上下文中的字符串——它们不进 UI，无需翻译
- 仅保留含中文字符的字符串
"""

import re, os

SRC = "src"
I18N = os.path.join(SRC, "i18n.rs")

# 语句上下文中出现这些模式即视为非 UI 文本
NON_UI_PAT = re.compile(
    r"\b(?:info|warn|error|debug|trace)!\s*[\(\[]"  # 日志宏
    r"|\bassert(?:_eq|_ne)?!\s*[\(\[]"  # 测试断言
    r"|\b(?:panic|unreachable|todo|unimplemented)!\s*[\(\[]"
    r"|\.\s*expect\s*\("  # panic 消息
    r"|#\s*\[\s*error\s*\("  # thiserror Display（UI 走 tr match 分支，Display 仅日志用）
)
CJK = re.compile(r"[㐀-䶿一-鿿]")


def strip_test_modules(code):
    """剔除 #[cfg(test)] 段（惯例位于文件尾，剔到下一个 #[cfg(test)] 或 EOF）。"""
    pat = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
    parts, idx = [], 0
    while True:
        m = pat.search(code, idx)
        if not m:
            parts.append(code[idx:])
            break
        parts.append(code[idx : m.start()])
        nxt = pat.search(code, m.end())
        idx = nxt.start() if nxt else len(code)
    return "".join(parts)


def strip_comments(code):
    out = []
    i, n = 0, len(code)
    while i < n:
        c = code[i]
        if c == '"':  # 普通字符串（处理转义）
            j = i + 1
            while j < n:
                if code[j] == "\\":
                    j += 2
                    continue
                if code[j] == '"':
                    break
                j += 1
            out.append(code[i : j + 1])
            i = j + 1
            continue
        if c == "r":  # raw string：r"..." 与 r#"..."#（支持多 #）
            k = i + 1
            hashes = 0
            while k < n and code[k] == "#":
                hashes += 1
                k += 1
            if k < n and code[k] == '"':
                end_tok = '"' + "#" * hashes
                j = code.find(end_tok, k + 1)
                j = n if j == -1 else j + len(end_tok)
                out.append(code[i:j])
                i = j
                continue
        if c == "/" and i + 1 < n and code[i + 1] == "/":
            while i < n and code[i] != "\n":
                i += 1
            continue
        if c == "/" and i + 1 < n and code[i + 1] == "*":
            i += 2
            while i + 1 < n and not (code[i] == "*" and code[i + 1] == "/"):
                i += 1
            i += 2
            continue
        out.append(c)
        i += 1
    return "".join(out)


def stmt_context(code, pos):
    """字符串起点 pos 之前、直到上一个语句边界（; { }）的代码片段。"""
    j = pos - 1
    while j >= 0 and code[j] not in ";{}":
        j -= 1
    return code[j + 1 : pos]


def ui_strings(code):
    res = []
    for m in re.finditer(r'"((?:[^"\\\n]|\\.)*)"', code):
        s = m.group(1)
        if not CJK.search(s):
            continue
        if NON_UI_PAT.search(stmt_context(code, m.start())):
            continue
        res.append(s)
    return res


# ---- 收集字典键 ----
i18n_code = open(I18N, encoding="utf-8").read()
i18n_stripped = strip_comments(i18n_code)
# match 头到文件尾
m = re.search(r"fn en_lookup.*?\{", i18n_stripped, re.S)
body = i18n_stripped[m.start() :]
keys = set(re.findall(r'"((?:[^"\\\n]|\\.)*)"\s*=>', body))

# ---- 收集每个文件 UI 可达字符串 ----
file_literals = {}
for root, _, files in os.walk(SRC):
    for f in files:
        if not f.endswith(".rs"):
            continue
        p = os.path.join(root, f)
        code = strip_test_modules(open(p, encoding="utf-8").read())
        stripped = strip_comments(code)
        lits = ui_strings(stripped)
        if lits:
            file_literals[p] = lits

missing = {}
for p, lits in file_literals.items():
    if p == I18N:
        continue
    gap = [l for l in lits if l not in keys]
    if gap:
        missing[p] = sorted(set(gap))

total = 0
for p, lits in sorted(missing.items()):
    print(f"== {p} ==")
    for l in lits:
        print(f"  {l}")
        total += 1
print(f"\n合计待审: {total}")

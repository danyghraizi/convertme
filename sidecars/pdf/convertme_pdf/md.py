"""PDF → Markdown: the page's document structure, without its line breaks.

Markdown is a flow format. The PDF's visual rows — the name and address centred
over a contact line, a right-aligned date beside an employer, four coursework
columns — become ordinary headings, paragraphs, lists and one table. Text is
never laid out with padding or hard line breaks, and a separator is written as
prose (`` — ``), not as an escaped table pipe.

The structure comes from `layout.frame`, so this writer and the DOCX/EPUB
writers agree about what a heading, an entry, a detail line, a project, a skill
row, a bullet and the coursework grid are. Raster icons are decoration for a
text file and are skipped; their labels and links are kept.
"""

from __future__ import annotations

import re
from dataclasses import replace
from pathlib import Path

from . import layout
from . import reflow

# Characters that would turn source prose into markup. Emphasis markers are the
# writer's, so a literal one in the PDF must be escaped before they are added.
_MARKDOWN_SPECIAL = re.compile(r"([\\`*_\[\]<>])")


def _escape_text(text: str) -> str:
    return _MARKDOWN_SPECIAL.sub(r"\\\1", text)


def _trimmed(runs: list[reflow.Run]) -> list[reflow.Run]:
    """Run copies without the line's leading/trailing whitespace."""
    copies = [replace(run) for run in runs]
    for run in copies:
        if run.text:
            run.text = (
                run.text.replace("\n", " ")
                .replace("\r", " ")
                .replace("\t", " ")
            )

    first = None
    for index, run in enumerate(copies):
        if run.image or (run.text and run.text.strip()):
            first = index
            break
    last = None
    for index in range(len(copies) - 1, -1, -1):
        run = copies[index]
        if run.image or (run.text and run.text.strip()):
            last = index
            break
    if first is None or last is None:
        return []
    copies = copies[first : last + 1]
    if copies[0].text:
        copies[0].text = copies[0].text.lstrip()
    if copies[-1].text:
        copies[-1].text = copies[-1].text.rstrip()
    return [run for run in copies if run.text]


def _inline(
    runs: list[reflow.Run],
    *,
    drop_bold: bool = False,
    drop_italic: bool = False,
    force_bold: bool = False,
    force_italic: bool = False,
    links: bool = True,
) -> str:
    """Markdown for a run list, merging adjacent runs with the same emphasis."""
    tokens: list[tuple[tuple[bool, bool, str | None], str]] = []
    for run in _trimmed(runs):
        if run.image or not run.text:
            continue
        bold = (run.bold or force_bold) and not drop_bold
        italic = (run.italic or force_italic) and not drop_italic
        link = run.link if links else None
        key = (bold, italic, link)
        if tokens and tokens[-1][0] == key:
            tokens[-1] = (key, tokens[-1][1] + run.text)
        else:
            tokens.append((key, run.text))

    out: list[str] = []
    for (bold, italic, link), text in tokens:
        if not text:
            continue
        if not text.strip():
            out.append(" ")
            continue
        core = _escape_text(text.strip())
        lead = text[: len(text) - len(text.lstrip())]
        trail = text[len(text.rstrip()) :]
        if bold and italic:
            core = f"***{core}***"
        elif bold:
            core = f"**{core}**"
        elif italic:
            core = f"*{core}*"
        if link:
            core = f"[{core}]({link})"
        out.append(lead + core + trail)
    return "".join(out)


def _plain(runs: list[reflow.Run]) -> str:
    """Whitespace-collapsed plain text (headings, table cells, dates)."""
    return " ".join("".join(run.text for run in runs).split())


def _split_runs(
    runs: list[reflow.Run], separator: str
) -> tuple[list[reflow.Run], list[reflow.Run]]:
    """Split a run list around the first separator character (the `|` or `:`)."""
    left: list[reflow.Run] = []
    right: list[reflow.Run] = []
    found = False
    for run in runs:
        if found:
            right.append(replace(run))
            continue
        if separator not in run.text:
            left.append(replace(run))
            continue
        head, _, tail = run.text.partition(separator)
        if head:
            left.append(replace(run, text=head))
        if tail:
            right.append(replace(run, text=tail))
        found = True
    return left, right


def _bullet_text(runs: list[reflow.Run]) -> str:
    text = _inline(runs).strip()
    while text and text[0] in reflow.BULLET_GLYPHS:
        text = text[1:].lstrip()
    return text


def _table(cells: list[list[list[reflow.Run]]]) -> str:
    columns = max((len(row) for row in cells), default=0)
    if not columns:
        return ""
    lines = [
        "| " + " | ".join([""] * columns) + " |",
        "| " + " | ".join(["---"] * columns) + " |",
    ]
    for row in cells:
        values = []
        for column in range(columns):
            value = _inline(row[column]) if column < len(row) else ""
            values.append(value.replace("|", "\\|"))
        lines.append("| " + " | ".join(values) + " |")
    return "\n".join(lines)


def _header_lines(header: layout.Header) -> list[str]:
    lines: list[str] = []
    title = _inline(header.title).strip()
    if title:
        lines.append(f"# {title}")
    for address in header.address:
        markup = _inline(address).strip()
        if markup:
            lines.append(markup)
    if header.contact:
        parts = []
        for segment in header.contact:
            markup = _inline(segment).strip()
            if markup:
                parts.append(markup)
        if parts:
            lines.append(" · ".join(parts))
    return lines


def _blocks_markdown(blocks: list[layout.Block]) -> str:
    chunks: list[str] = []
    list_items: list[str] = []

    def flush_list() -> None:
        if list_items:
            chunks.append("\n".join(list_items))
            list_items.clear()

    for block in blocks:
        if block.kind not in ("bullet", "skill"):
            flush_list()

        if block.kind == "heading":
            level = 2 if block.level <= 1 else 3
            markup = _inline(block.runs, drop_bold=True).strip()
            if markup:
                chunks.append("#" * level + " " + markup)
            continue

        if block.kind == "entry":
            name = _inline(block.left, drop_bold=True).strip()
            date = _inline(block.right, drop_bold=True).strip()
            if name:
                chunks.append(f"### {name}" + (f" — {date}" if date else ""))
            continue

        if block.kind == "detail":
            role = _inline(block.left, force_italic=True, drop_bold=True).strip()
            place = _inline(
                block.right, drop_bold=True, drop_italic=True
            ).strip()
            if role:
                chunks.append(role + (f" — {place}" if place else ""))
            continue

        if block.kind == "project":
            name_runs, tech_runs = _split_runs(block.left, "|")
            name = _inline(
                name_runs, force_bold=True, drop_italic=True
            ).strip()
            tech = _inline(
                tech_runs, force_italic=True, drop_bold=True
            ).strip()
            date = _inline(
                block.right, drop_bold=True, drop_italic=True
            ).strip()
            if name:
                line = f"### {name}"
                if tech:
                    line += f" — {tech}"
                if date:
                    line += f" — {date}"
                chunks.append(line)
            continue

        if block.kind == "skill":
            label_runs, rest_runs = _split_runs(block.runs, ":")
            label = _inline(
                label_runs, force_bold=True, drop_italic=True
            ).strip()
            rest = _inline(rest_runs).strip()
            if label:
                list_items.append(f"- {label}: {rest}" if rest else f"- {label}")
            continue

        if block.kind == "bullet":
            text = _bullet_text(block.runs)
            if text:
                list_items.append(f"- {text}")
            continue

        if block.kind == "grid":
            table = _table(block.cells)
            if table:
                chunks.append(table)
            continue

        if block.kind == "picture":
            continue

        markup = _inline(block.runs).strip()
        if markup:
            chunks.append(markup)

    flush_list()
    return "\n\n".join(chunks)


def convert(source: str, target: str) -> None:
    import pymupdf as fitz

    with fitz.open(source) as document:
        if document.page_count == 0:
            raise ValueError("the PDF has no pages")
        layout_data = reflow.measure(document)
        pages = [layout.frame(page, layout_data) for page in document]
        metadata = document.metadata or {}
        fallback_title = str(metadata.get("title") or "").strip() or Path(source).stem

    first_header = pages[0][0]
    header = _header_lines(first_header)
    if not any(line.startswith("# ") for line in header):
        fallback = _plain(first_header.title) or fallback_title
        header.insert(0, f"# {_escape_text(fallback)}")
    sections: list[str] = ["\n\n".join(header)]

    for _page_header, blocks, _rows in pages:
        # A running letterhead on later pages repeats the document heading; the
        # first page already carries it.
        blocks_text = _blocks_markdown(blocks)
        if blocks_text:
            sections.append(blocks_text)

    markdown = "\n\n".join(section for section in sections if section)
    Path(target).write_text(markdown + "\n", encoding="utf-8")

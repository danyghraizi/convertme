"""Turn a PDF page into paragraphs a writer (DOCX, EPUB) can emit.

This is the part that decides *structure*: which line is a heading, which is a bullet,
where a paragraph continues, and what is an icon rather than text. Both writers read
the same result, so `pdf → docx` and `pdf → epub` cannot describe the document
differently.

Rules, and the reasons they are rules:

* Body size is the size carrying the most characters. Everything above it by 8 % is a
  heading size; the very largest is the document's title (the name on a résumé), and the
  remaining heading sizes map to Heading 1, 2, 3 in descending order.
* A line is a heading only if it is short (≤ 80 characters) and the block is short
  (≤ 3 lines): a long paragraph typeset at a heading size is still a paragraph.
* A block whose first line is entirely bold at body size is a sub-entry — an employer,
  a school — and its first line becomes Heading 2. That is how a two-column résumé
  header ("State University / City, State") keeps its shape.
* A line starting with a bullet glyph becomes a list item; a line that does not start a
  new bullet but continues in lowercase is appended to the paragraph before it, which is
  what a wrapped bullet looks like after extraction.
* Spans carry their own bold/italic and hyperlink; a space is added between spans only
  where the PDF already had one, because inserting spaces inside a word ("Java Script")
  changes the text the document says it contains.
* A span made only of symbol-font glyphs is an icon. It is rasterised, not written as
  text — otherwise the résumé's phone and envelope become tofu characters.
"""

from __future__ import annotations

from dataclasses import dataclass, field

BULLET_GLYPHS = "•●▪◦‣∙·"
BULLET_PREFIX = ("-", "–", "—", "*")
# Below this, a span of symbol glyphs is an icon worth rasterising.
ICON_MAX_PT = 14.0
# Icons smaller than this are noise; the plan says skip under 8×8 px.
ICON_MIN_PX = 8
BODY_HEADING_FACTOR = 1.08
MAX_HEADING_CHARS = 80
MAX_HEADING_LINES = 3


@dataclass
class Run:
    text: str
    bold: bool = False
    italic: bool = False
    size: float = 0.0
    link: str | None = None
    image: bytes | None = None


@dataclass
class Paragraph:
    style: str  # Body Text, Heading 1..3, List Bullet, Title
    runs: list[Run] = field(default_factory=list)
    align: str = "left"
    # Where the paragraph sits on the page: the writer interleaves tables by it.
    y: float = 0.0

    @property
    def text(self) -> str:
        return "".join(run.text for run in self.runs)


@dataclass
class TableSpec:
    rows: list[list[str]]
    y: float = 0.0


@dataclass
class PageSpec:
    paragraphs: list[Paragraph]
    tables: list[TableSpec]


@dataclass
class Layout:
    """Everything decided once for the whole document."""

    body_size: float = 10.0
    body_font: str = "Calibri"
    title_size: float | None = None
    heading_levels: dict[float, int] = field(default_factory=dict)


def _is_icon(char: str) -> bool:
    code = ord(char)
    # Private use area and the symbol blocks a typesetter uses for contact icons.
    return (
        0xE000 <= code <= 0xF8FF
        or 0xF000 <= code <= 0xF0FF
        or code in (0x83, 0x8A, 0x8F, 0x9A, 0xA7, 0xB7)
        or (0x0370 <= code <= 0x03FF and char in "ï§")
    )


def _span_is_icon(text: str) -> bool:
    """A contact icon: a very short span with no letters or digits in it.

    Typesetters put the phone/envelope/link marks in a symbol font, so they extract as
    one or two odd characters. Writing those as text produces tofu; they are rasterised
    instead. A span with any letter or digit is text, however odd it looks.
    """
    stripped = text.strip()
    if not stripped or len(stripped) > 3:
        return False
    # A bullet is a list marker, not an icon: rasterising it turned every list item
    # into a picture and lost the list.
    if any(char in BULLET_GLYPHS for char in stripped):
        return False
    return not any(char.isalnum() for char in stripped)


def _dominant_size(spans) -> float:
    counts: dict[float, int] = {}
    for span in spans:
        size = round(float(span.get("size", 0.0)), 1)
        counts[size] = counts.get(size, 0) + len(span.get("text", ""))
    if not counts:
        return 0.0
    return max(counts.items(), key=lambda item: item[1])[0]


def measure(pdf) -> Layout:
    """Read the whole document once to decide body size, title and heading levels."""
    sizes: dict[float, int] = {}
    fonts: dict[str, int] = {}
    for page in pdf:
        data = page.get_text("dict", sort=True)
        for block in data.get("blocks", []):
            for line in block.get("lines", []):
                for span in line.get("spans", []):
                    text = span.get("text", "")
                    if not text.strip() or _span_is_icon(text):
                        continue
                    size = round(float(span.get("size", 0.0)), 1)
                    sizes[size] = sizes.get(size, 0) + len(text)
                    font = span.get("font", "")
                    if font:
                        fonts[font] = fonts.get(font, 0) + len(text)

    layout = Layout()
    if sizes:
        layout.body_size = max(sizes.items(), key=lambda item: item[1])[0]
    if fonts:
        layout.body_font = max(fonts.items(), key=lambda item: item[1])[0]

    heading_sizes = sorted(
        (size for size in sizes if size > layout.body_size * BODY_HEADING_FACTOR),
        reverse=True,
    )
    if heading_sizes:
        # The largest size is the document title when it is only a handful of
        # characters (a name, a company); a page's section headings are the next level.
        largest = heading_sizes[0]
        if sizes.get(largest, 0) <= 40 and len(heading_sizes) > 1:
            layout.title_size = largest
            heading_sizes = heading_sizes[1:]
        for rank, size in enumerate(heading_sizes[:3]):
            layout.heading_levels[size] = rank + 1
    return layout


def _bullet_text(line_text: str) -> tuple[bool, str]:
    text = line_text.strip()
    if not text:
        return False, text
    if text[0] in BULLET_GLYPHS:
        return True, text[1:].strip()
    for prefix in BULLET_PREFIX:
        if text.startswith(prefix + " "):
            return True, text[len(prefix) + 1 :].strip()
    return False, text


def _links_for(page) -> list[tuple[object, str]]:
    rects = []
    for link in page.get_links():
        uri = link.get("uri")
        rect = link.get("from")
        if uri and rect is not None:
            rects.append((rect, uri))
    return rects


def _link_at(rects, bbox):
    for rect, uri in rects:
        if rect is None:
            continue
        try:
            if rect.intersects(bbox):
                return uri
        except (AttributeError, TypeError, ValueError):
            continue
    return None


def _rasterise(page, rect) -> bytes | None:
    """An icon's pixels, at a size worth embedding."""
    try:
        pixmap = page.get_pixmap(clip=rect, dpi=600, alpha=False)
    except Exception:  # noqa: BLE001 - a missing glyph must not fail the export
        return None
    if pixmap.width < ICON_MIN_PX or pixmap.height < ICON_MIN_PX:
        return None
    return pixmap.tobytes("png")


def _runs_for_line(page, line, layout: Layout, links) -> list[Run]:
    runs: list[Run] = []
    for span in line.get("spans", []):
        text = span.get("text", "")
        if not text:
            continue
        size = round(float(span.get("size", 0.0)), 1)
        if _span_is_icon(text) and size <= ICON_MAX_PT:
            import pymupdf

            rect = pymupdf.Rect(span.get("bbox"))
            # A little breathing room: glyph boxes are tight and the icon's own
            # antialiasing lives just outside them.
            rect = rect + (-0.6, -0.6, 0.6, 0.6)
            image = _rasterise(page, rect)
            if image is not None:
                runs.append(Run(text="", size=size, image=image))
                continue
        flags = int(span.get("flags", 0))
        runs.append(
            Run(
                text=text,
                bold=bool(flags & 16),
                italic=bool(flags & 2),
                size=size,
                link=_link_at(links, span.get("bbox")),
            )
        )
    return runs


def _join_runs(runs: list[Run]) -> list[Run]:
    """A line's runs, in order, exactly as the PDF spaced them.

    No space is inserted between spans: this PDF keeps the spaces as their own spans,
    and inventing one where the file has none splits words ("Java Script"). Where a
    typesetter did omit a space between two spans, the missing space is the PDF's
    statement about the text, not ours to correct.
    """
    return [run for run in runs if run.text or run.image]


def acceptable_tables(page) -> list[TableSpec]:
    """Tables worth writing.

    PDF table detection finds a grid in plenty of things that are not tables — a
    two-column résumé header reads as a 4×3 table with six empty cells, and taking it
    put the whole education section into table cells. A real table has at least two
    rows, at least two columns, and a cell filled in every column.
    """
    tables: list[TableSpec] = []
    try:
        finder = page.find_tables()
    except Exception:  # noqa: BLE001 - detection is best effort
        return tables
    for table in finder.tables or []:
        rows = table.extract()
        if not rows:
            continue
        columns = max(len(row) for row in rows)
        if len(rows) < 2 or columns < 2:
            continue
        filled = [
            [bool((row[c] or "").strip()) if c < len(row) else False for c in range(columns)]
            for row in rows
        ]
        # Every column must be used, and every row must fill at least two cells.
        if not all(any(row[c] for row in filled) for c in range(columns)):
            continue
        if not all(sum(row) >= 2 for row in filled):
            continue
        tables.append(TableSpec(rows=rows, y=float(table.bbox[1])))
    return tables


def page_spec(page, layout: Layout, skip_rects=()) -> PageSpec:
    """A page's paragraphs, in reading order, with its tables."""

    import pymupdf

    links = _links_for(page)
    paragraphs: list[Paragraph] = []
    tables: list[TableSpec] = []
    current: Paragraph | None = None
    block_lines_seen = 0
    block_size = 0.0

    def flush():
        nonlocal current
        if current is not None and current.text.strip():
            paragraphs.append(current)
        current = None

    for block_index, block in enumerate(page.get_text("dict", sort=True).get("blocks", [])):
        if block.get("type") == 1:
            flush()
            # A picture block: embed it as its own paragraph.
            try:
                image = pymupdf.Pixmap(page.parent, block["number"])
                if image.n - image.alpha >= 4:
                    image = pymupdf.Pixmap(pymupdf.csRGB, image)
                if image.width >= ICON_MIN_PX and image.height >= ICON_MIN_PX:
                    paragraphs.append(
                        Paragraph(style="Picture", runs=[Run(text="", image=image.tobytes("png"))])
                    )
            except Exception:  # noqa: BLE001 - a broken image must not fail the export
                pass
            continue

        lines = block.get("lines", [])
        block_lines_seen = len(lines)
        block_size = _dominant_size(
            (span for line in lines for span in line.get("spans", []))
        )
        first_line = lines[0] if lines else None
        first_runs = _runs_for_line(page, first_line, layout, links) if first_line else []
        first_text = "".join(run.text for run in first_runs).strip()
        first_bold = bool(first_runs) and all(
            run.bold for run in first_runs if run.text.strip() and run.image is None
        )
        # A heading is a line, not a block: a section title can arrive inside a block
        # that also holds the entries under it, and judging by the block's dominant
        # size then turned every section title into body text.
        def heading_level(size: float) -> int | None:
            rounded = round(size, 1)
            if rounded in layout.heading_levels:
                return layout.heading_levels[rounded]
            if layout.title_size is not None and abs(rounded - layout.title_size) < 0.05:
                return 0
            return None

        if any(pymupdf.Rect(block["bbox"]).intersects(rect) for rect in skip_rects):
            continue

        for line in lines:
            if any(pymupdf.Rect(line["bbox"]).intersects(rect) for rect in skip_rects):
                continue
            runs = _join_runs(_runs_for_line(page, line, layout, links))
            text = "".join(run.text for run in runs)
            if not text.strip() and not any(run.image for run in runs):
                continue

            bullet, stripped = _bullet_text(text)
            size = round(float(line["spans"][0].get("size", 0.0)), 1) if line.get("spans") else 0.0
            line_bold = all(
                run.bold for run in runs if run.text.strip() and run.image is None
            ) and any(run.text.strip() for run in runs)

            line_y = float(line["bbox"][1]) if line.get("bbox") else 0.0

            if bullet:
                flush()
                paragraphs.append(
                    Paragraph(style="List Bullet", runs=[_strip_prefix(runs)], y=line_y)
                )
                continue

            line_size = _dominant_size(line.get("spans", []))
            level = heading_level(line_size)
            if level is not None and len(stripped) <= MAX_HEADING_CHARS:
                flush()
                style = "Title" if level == 0 else f"Heading {level}"
                paragraphs.append(Paragraph(style=style, runs=runs, y=line_y))
                continue

            # A bold first line of a body-size block is a sub-entry (an employer,
            # a school): Heading 2, with the block's remaining lines as its body.
            if (
                first_bold
                and line is first_line
                and len(first_text) <= MAX_HEADING_CHARS
                and abs(block_size - layout.body_size) < 0.05
                and line_bold
            ):
                flush()
                paragraphs.append(Paragraph(style="Heading 2", runs=runs, y=line_y))
                continue

            if current is None:
                current = Paragraph(style="Body Text", runs=list(runs), y=line_y)
            else:
                # A continuation inside the same flow joins with one space.
                if current.runs and not current.text.endswith(" "):
                    current.runs[-1].text += " "
                current.runs.extend(runs)

    flush()
    return PageSpec(paragraphs=paragraphs, tables=tables)


def _strip_prefix(runs: list[Run]) -> Run:
    """The list item's text, without the bullet glyph it was attached to."""
    merged = Run(
        text="",
        bold=any(run.bold for run in runs),
        italic=any(run.italic for run in runs),
        size=runs[0].size if runs else 0.0,
        link=runs[0].link if runs else None,
    )
    parts = []
    for run in runs:
        parts.append(run.text)
    text = "".join(parts).strip()
    if text and text[0] in BULLET_GLYPHS:
        text = text[1:].lstrip()
    merged.text = text
    return merged


def alignment_for(runs: list[Run], page_rect, block_bbox) -> str:
    """A short block centred on the page stays centred."""
    try:
        left, top, right, bottom = block_bbox
        page_mid = (page_rect.x0 + page_rect.x1) / 2
        block_mid = (left + right) / 2
        if abs(block_mid - page_mid) <= page_rect.width * 0.05:
            return "center"
    except (TypeError, ValueError, AttributeError):
        pass
    return "left"

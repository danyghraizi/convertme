"""PDF → XLSX: one sheet per page, laid out in four read columns.

A spreadsheet is not a document, so the writer does not put the page's text into a
single column (or a single cell). It reuses ``layout.frame`` — the same visual rows
the DOCX writer uses — and maps them to four bands:

* A is the page's left column and carries headings, bullets, skills and the left
  half of an employer/school line;
* D is the right column and carries the date or the place beside that line;
* B and C are the middle bands, used by the four-column coursework grid;
* a section heading and body paragraph span A:D so the fill reads as a section.

Every heading, employer, date and bullet is its own cell, wrapped and sized so the
sheet is readable without opening the original PDF. A detected table still gets its
own sheet, with a bold header row, so a truly tabular PDF is not flattened into one
column either.
"""

from __future__ import annotations

import math
import re

BANDS = 4
# A is the wide text column; D holds a date ("May 2020 – August 2020").
COLUMN_WIDTHS = (48, 20, 22, 26)
HEADING_FILL = "DDEBF7"
TABLE_HEADER_FILL = "E8EEF7"
TABLE_RULE = "B7C4D6"
BODY_PT = 10.5
LINE_PT = 14.5
MIN_ROW_PT = 16.0
MAX_ROW_PT = 180.0
BLOB_CHARS = 280

_ICON_FONTS = ("awesome", "wingding", "dingbat", "symbol", "material", "icon")
_TOFU = frozenset("\x83\u00a7\u00ef")
_CONTROL = frozenset(
    list(range(0, 9)) + [11, 12] + list(range(14, 32)) + list(range(127, 160))
)
_SPACES = re.compile(r"\s+")
_LEADING_ICON = re.compile(r"^[\u00a7\u00ef]\s*")


def _run_text(run) -> str:
    """A run's text, with icon-font glyphs and control characters removed.

    The PDF's phone/envelope/link marks are rasterised by ``reflow``, so an icon run
    usually carries only an image. A font that still extracts a letter-shaped glyph
    (FontAwesome's LinkedIn mark is "ï") is dropped instead of becoming tofu.
    """
    if run.image:
        return ""
    text = run.text or ""
    if any(marker in (run.font or "").lower() for marker in _ICON_FONTS):
        return ""
    text = "".join(char for char in text if ord(char) not in _CONTROL)
    stripped = text.strip()
    if stripped and all(char in _TOFU for char in stripped):
        return ""
    return text


def _text(runs) -> str:
    return _SPACES.sub(" ", "".join(_run_text(run) for run in runs)).strip()


def _rich(runs, base_bold: bool = False, base_italic: bool = False):
    """A cell value that keeps the PDF's bold/italic runs where they matter.

    A plain string is returned when no run differs from the cell's font, so simple
    rows stay ordinary cells instead of rich text.
    """
    from openpyxl.cell.rich_text import CellRichText, TextBlock
    from openpyxl.cell.text import InlineFont

    parts: list[tuple[str, bool, bool]] = []
    for run in runs:
        text = _run_text(run)
        if not text:
            continue
        bold = bool(run.bold) or base_bold
        italic = bool(run.italic) or base_italic
        parts.append((text, bold, italic))
    if not parts:
        return ""
    first, bold, italic = parts[0]
    parts[0] = (first.lstrip(), bold, italic)
    last, bold, italic = parts[-1]
    parts[-1] = (last.rstrip(), bold, italic)
    parts = [part for part in parts if part[0]]
    if not parts:
        return ""
    if not any(bold or italic for _, bold, italic in parts):
        return "".join(text for text, _, _ in parts)
    blocks = []
    for text, bold, italic in parts:
        if bold or italic:
            blocks.append(
                TextBlock(
                    InlineFont(rFont="Calibri", sz=BODY_PT, b=bold or None, i=italic or None),
                    text,
                )
            )
        else:
            blocks.append(text)
    return CellRichText(blocks)


def _prefixed(runs, prefix: str):
    """The cell value for a bullet: the text with its glyph, never the glyph alone."""
    from . import reflow

    # layout strips the marker but leaves its trailing space, so strip the first
    # text run here or every grid cell would read "•  Data Structures".
    clean = []
    stripped = False
    for run in runs:
        clone = reflow.Run(**{**run.__dict__})
        if not stripped and clone.text:
            clone.text = clone.text.lstrip()
            stripped = bool(clone.text)
        clean.append(clone)
    return _rich([reflow.Run(text=prefix)] + clean)


def _height(text: str, width: float) -> float:
    """How tall a wrapped cell needs to be, in points."""
    if not text:
        return MIN_ROW_PT
    lines = 0
    for part in str(text).splitlines() or [""]:
        lines += max(1, math.ceil(len(part) / max(8.0, width * 0.98)))
    return min(MAX_ROW_PT, max(MIN_ROW_PT, lines * LINE_PT + 2.5))


def _set_height(sheet, row: int, needed: float) -> None:
    current = sheet.row_dimensions[row].height
    if current is None or needed > current:
        sheet.row_dimensions[row].height = min(MAX_ROW_PT, needed)


def _width(sheet, first: int = 1, last: int = BANDS) -> float:
    letters = "ABCD"
    return sum(
        sheet.column_dimensions[letters[column - 1]].width or 0
        for column in range(first, min(last, BANDS) + 1)
    )


def _style(
    cell,
    *,
    bold: bool = False,
    italic: bool = False,
    size: float = BODY_PT,
    align: str = "left",
    fill: str | None = None,
    wrap: bool = True,
    border: bool = False,
    color: str = "000000",
) -> None:
    from openpyxl.styles import Alignment, Border, Font, PatternFill, Side

    cell.font = Font(name="Calibri", size=size, bold=bold, italic=italic, color=color)
    cell.alignment = Alignment(
        wrap_text=wrap, vertical="top", horizontal=align
    )
    if fill:
        cell.fill = PatternFill("solid", fgColor=fill)
    if border:
        side = Side(style="thin", color=TABLE_RULE)
        cell.border = Border(bottom=side)


def _merge(sheet, row: int, first: int = 1, last: int = BANDS) -> None:
    if last > first:
        sheet.merge_cells(
            start_row=row, start_column=first, end_row=row, end_column=last
        )


def _set_widths(sheet, widths=COLUMN_WIDTHS) -> None:
    for letter, width in zip("ABCD", widths):
        sheet.column_dimensions[letter].width = width


def _write_header(sheet, header, row: int) -> int:
    """Title, address lines and the contact row, spanning the four bands."""
    from openpyxl.styles import Alignment

    total = _width(sheet)
    if header.title:
        _merge(sheet, row)
        cell = sheet.cell(row=row, column=1, value=_text(header.title))
        _style(cell, bold=True, size=16)
        _set_height(sheet, row, 26.0)
        row += 1

    for address in header.address:
        text = _text(address)
        if not text:
            continue
        _merge(sheet, row)
        cell = sheet.cell(row=row, column=1, value=text)
        _style(cell, size=11)
        _set_height(sheet, row, _height(text, total))
        row += 1

    contacts = [text for text in (_text(segment) for segment in header.contact) if text]
    if contacts:
        # The icons are rasterised, so only the words remain. Put the contact items
        # across the page's four bands in their left-to-right order.
        for offset, text in enumerate(contacts[:BANDS]):
            cell = sheet.cell(row=row, column=offset + 1, value=text)
            _style(cell, size=10.5)
            _set_height(
                sheet,
                row,
                _height(text, sheet.column_dimensions["ABCD"[offset]].width or 10),
            )
        if len(contacts) > BANDS:
            last = sheet.cell(row=row, column=BANDS)
            joined = " | ".join(contacts[BANDS - 1 :])
            last.value = joined
            last.alignment = Alignment(wrap_text=True, vertical="top")
            _set_height(
                sheet,
                row,
                _height(joined, sheet.column_dimensions["D"].width or 10),
            )
        row += 1
    return row


def _write_heading(sheet, block, row: int) -> int:
    _merge(sheet, row)
    text = _text(block.runs)
    for column in range(1, BANDS + 1):
        cell = sheet.cell(row=row, column=column)
        _style(
            cell,
            bold=True,
            size=11.5,
            fill=HEADING_FILL,
            border=bool(block.border),
        )
    sheet.cell(row=row, column=1, value=text)
    _set_height(sheet, row, max(20.0, _height(text, _width(sheet))))
    return row + 1


def _write_right_line(sheet, block, row: int, *, italic: bool = False) -> int:
    """An employer/school/project line: left text in A:C, date or place in D."""
    left = _rich(block.left)
    right = _rich(block.right)
    _merge(sheet, row, 1, 3)
    left_cell = sheet.cell(row=row, column=1, value=left)
    _style(left_cell, size=10.5, italic=italic)
    right_cell = sheet.cell(row=row, column=4, value=right)
    _style(right_cell, size=10, align="right", italic=italic)
    left_text = str(left)
    right_text = str(right)
    _set_height(
        sheet,
        row,
        max(_height(left_text, _width(sheet, 1, 3)), _height(right_text, _width(sheet, 4, 4))),
    )
    return row + 1


def _write_grid(sheet, block, row: int) -> int:
    for cells in block.cells:
        tallest = MIN_ROW_PT
        for column in range(BANDS):
            runs = cells[column] if column < len(cells) else []
            value = _prefixed(runs, "• ") if runs and _text(runs) else ""
            cell = sheet.cell(row=row, column=column + 1, value=value)
            _style(cell, size=10.5)
            tallest = max(
                tallest,
                _height(str(value), sheet.column_dimensions["ABCD"[column]].width or 10),
            )
        _set_height(sheet, row, tallest)
        row += 1
    return row


def _write_block(sheet, block, row: int) -> int:
    if block.kind == "heading":
        return _write_heading(sheet, block, row)
    if block.kind in ("entry", "project"):
        return _write_right_line(sheet, block, row)
    if block.kind == "detail":
        return _write_right_line(sheet, block, row, italic=True)
    if block.kind == "grid":
        return _write_grid(sheet, block, row)
    if block.kind == "bullet":
        cell = sheet.cell(
            row=row, column=1, value=_prefixed(block.runs, "• ")
        )
        _style(cell)
        _set_height(sheet, row, _height(str(cell.value), _width(sheet, 1, 1)))
        return row + 1
    if block.kind == "skill":
        cell = sheet.cell(row=row, column=1, value=_rich(block.runs))
        _style(cell)
        _set_height(sheet, row, _height(str(cell.value), _width(sheet, 1, 1)))
        return row + 1
    if block.kind == "body":
        value = _rich(block.runs)
        _merge(sheet, row)
        cell = sheet.cell(row=row, column=1, value=value)
        _style(cell)
        _set_height(sheet, row, _height(str(value), _width(sheet)))
        return row + 1
    # A picture is not a value in a spreadsheet; the page sheet carries the text and
    # the table sheet (when there is one) keeps the numbers.
    return row


def _page_sheet(workbook, page_number: int, header, blocks):
    sheet = workbook.create_sheet(f"Page {page_number}")
    _set_widths(sheet)
    row = _write_header(sheet, header, 1)
    content_row = row
    for block in blocks:
        row = _write_block(sheet, block, row)
    if row == 1:
        sheet.cell(row=1, column=1, value="(blank page)")
        _style(sheet.cell(row=1, column=1), italic=True, color="808080")
    elif row > content_row:
        sheet.freeze_panes = f"A{content_row}"
    sheet.sheet_view.zoomScale = 90
    return sheet


def _table_value(value) -> str:
    if value is None:
        return ""
    lines = []
    for raw in str(value).splitlines():
        line = _SPACES.sub(" ", raw).strip()
        line = "".join(char for char in line if ord(char) not in _CONTROL)
        line = _LEADING_ICON.sub("", line).strip()
        if not line or all(char in _TOFU for char in line):
            continue
        if all(char in "•●▪◦‣∙·" for char in line):
            continue
        # PyMuPDF breaks a dash or a trailing comma onto its own line; put it back
        # with the phrase it belongs to so the table cell reads as one value.
        if line in ("–", "—", "-") and lines:
            lines[-1] = lines[-1].rstrip() + " " + line
            continue
        if line in (",", ";", ".") and lines:
            lines[-1] += line
            continue
        lines.append(line)
    return "\n".join(lines)


def _table_sheet(workbook, name: str, rows):
    """A detected table as its own sheet: cells stay cells, header row bold."""
    values = [[_table_value(value) for value in row] for row in rows]
    columns = max((len(row) for row in values), default=0)
    if not columns:
        return None
    sheet = workbook.create_sheet(name[:31])
    widths = []
    for column in range(columns):
        longest = max(
            (
                max((len(line) for line in row[column].splitlines()), default=0)
                for row in values
                if column < len(row)
            ),
            default=8,
        )
        widths.append(min(40.0, max(12.0, longest + 2)))
    for column, width in enumerate(widths, start=1):
        sheet.column_dimensions[
            sheet.cell(row=1, column=column).column_letter
        ].width = width

    for number, row in enumerate(values, start=1):
        tallest = MIN_ROW_PT
        for column, value in enumerate(row, start=1):
            cell = sheet.cell(row=number, column=column, value=value)
            _style(
                cell,
                bold=number == 1,
                size=10.5,
                fill=TABLE_HEADER_FILL if number == 1 else None,
                border=number == 1,
            )
            tallest = max(tallest, _height(value, widths[column - 1]))
        _set_height(sheet, number, tallest)
    if len(values) > 1:
        sheet.freeze_panes = "A2"
    sheet.sheet_view.zoomScale = 90
    return sheet


def _blob(rows) -> bool:
    """A "table" that is one huge cell is not a table worth a separate sheet."""
    cells = [
        str(cell).strip()
        for row in rows
        for cell in row
        if cell is not None and str(cell).strip()
    ]
    return not cells or (len(cells) == 1 and len(cells[0]) > BLOB_CHARS)


def _real_table(rows) -> bool:
    """Reject a table guess whose rows do not fill at least two cells.

    On the résumé PyMuPDF finds a 4×3 "table" around Education and Coursework:
    one row is just "Relevant Coursework", another only the left degree, and the
    cells contain mangled copies of lines the page sheet already has. A real
    table fills at least two columns in every row; this guess does not.
    """
    values = [[_table_value(cell) for cell in row] for row in rows]
    if len(values) < 2:
        return False
    if max((len(row) for row in values), default=0) < 2:
        return False
    return all(
        sum(1 for value in row if value.strip()) >= 2 for row in values
    )


def _table_only_page(header, blocks, spans) -> bool:
    """A page that is only a table, with no header or section text of its own.

    Its table sheets are the page; writing the same rows again as merged body
    paragraphs would put "Name Role Score" in one cell above a readable table.
    A block outside every detected table — a title above it, say — keeps the page
    sheet, and a résumé page always has a title or headings.
    """
    if not (
        not header.title
        and not header.address
        and not header.contact
        and bool(blocks)
        and bool(spans)
        and all(block.kind == "body" for block in blocks)
    ):
        return False
    for block in blocks:
        if not any(
            span is not None and span[0] - 6 <= block.y <= span[1] + 6
            for span in spans
        ):
            return False
    return True


def convert(source: str, target: str) -> None:
    import pymupdf as fitz
    from openpyxl import Workbook

    with fitz.open(source) as document:
        if document.page_count == 0:
            raise ValueError("the PDF has no pages")
        from . import layout as layout_mod
        from . import reflow

        layout_data = reflow.measure(document)
        workbook = Workbook()
        workbook.remove(workbook.active)

        for page_number, page in enumerate(document, start=1):
            header, blocks, _rows = layout_mod.frame(page, layout_data)

            # Table detection is kept: a genuine table gets its own cells and a bold
            # header. On a résumé PyMuPDF can call the education block a table and its
            # cells read as one multi-line blob, so the page sheet remains the
            # readable view there; a page that is only a table uses its table sheets.
            try:
                finder = page.find_tables()
                tables = [table for table in (finder.tables or [])]
            except Exception:  # noqa: BLE001 - table detection is best effort
                tables = []

            table_rows = []
            table_spans = []
            for table in tables:
                try:
                    rows = table.extract()
                except Exception:  # noqa: BLE001 - a broken table must not fail the export
                    continue
                if not rows or _blob(rows) or not _real_table(rows):
                    continue
                bbox = getattr(table, "bbox", None)
                table_rows.append(rows)
                if bbox is None:
                    table_spans.append(None)
                elif hasattr(bbox, "y0"):
                    table_spans.append((float(bbox.y0), float(bbox.y1)))
                else:
                    table_spans.append((float(bbox[1]), float(bbox[3])))

            table_only = bool(table_rows) and _table_only_page(
                header, blocks, table_spans
            )
            if not table_only:
                _page_sheet(workbook, page_number, header, blocks)
            for table_number, rows in enumerate(table_rows, start=1):
                # A page that is only one table is that table: keep the page's name
                # rather than inventing a second, text-shaped sheet beside it.
                if table_only and len(table_rows) == 1:
                    name = f"Page {page_number}"
                else:
                    name = f"Page {page_number} Table {table_number}"
                _table_sheet(workbook, name, rows)

        if not workbook.worksheets:
            raise ValueError("the PDF has no pages")
        workbook.save(target)

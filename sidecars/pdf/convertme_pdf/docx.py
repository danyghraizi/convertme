"""PDF → DOCX with the page's own visual layout.

The previous writer walked PyMuPDF text blocks. A block does not mean a paragraph:
on a two-column résumé it held the left employer and the right date, another held
four coursework columns, and another held the last bullet of a section followed by
the next heading. The result merged dates into degrees, ordered coursework down
columns, split wrapped bullets, and put the photo and icons out of flow.

`layout.py` flattens the page into visual rows. This writer turns a row into the
Word shape it actually is:

* a titled, indented header with the photo and inline contact icons;
* section headings with the PDF's horizontal rule as a bottom border;
* employer/school lines with a right-aligned date tab;
* italic detail lines with a right-aligned place;
* project lines (bold name, italic technology, right-aligned date);
* real bullet lists, including the four-column coursework table;
* body paragraphs that join only the lines of one paragraph.

The page size, margins, 10 pt Calibri body and 12 pt bold headings match the
reference PDF-to-DOCX result, so the document fits the page instead of spilling
into a second one with a black band between.
"""

from __future__ import annotations

import io

from . import layout
from . import reflow

BLACK = "000000"
BODY_PT = 10.0
HEADING_1_PT = 12.0
HEADING_2_PT = 10.0


def _hyperlink(paragraph, url: str, run_specs: list[reflow.Run]) -> None:
    """A real `w:hyperlink` run, black and underlined like the PDF's link text."""
    from docx.oxml.ns import qn
    from docx.oxml import OxmlElement
    from docx.shared import Pt, RGBColor
    from docx.text.run import Run as DocxRun

    relationship = paragraph.part.relate_to(
        url,
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink",
        is_external=True,
    )
    hyperlink = OxmlElement("w:hyperlink")
    hyperlink.set(qn("r:id"), relationship)
    paragraph._p.append(hyperlink)
    for spec in run_specs:
        run = DocxRun(_new_run_element(hyperlink), paragraph)
        run.text = spec.text
        if spec.size:
            run.font.size = Pt(min(24.0, max(6.0, spec.size)))
        run.font.bold = spec.bold
        run.font.italic = spec.italic
        run.font.color.rgb = RGBColor(0, 0, 0)
        run.font.underline = True


def _new_run_element(parent):
    from docx.oxml import OxmlElement

    element = OxmlElement("w:r")
    parent.append(element)
    return element


def _list_numbering(document) -> int:
    """A bullet `numId` this document owns, so list items carry real numbering."""
    from docx.oxml import OxmlElement
    from docx.oxml.ns import qn

    try:
        numbering = document.part.numbering_part.element
    except (KeyError, NotImplementedError, AttributeError):
        return 0
    ids = [int(num.get(qn("w:numId"))) for num in numbering.findall(qn("w:num"))]
    next_id = (max(ids) + 1) if ids else 1

    abstract_ids = [
        int(item.get(qn("w:abstractNumId")))
        for item in numbering.findall(qn("w:abstractNum"))
    ]
    abstract_id = (max(abstract_ids) + 1) if abstract_ids else 0

    abstract = OxmlElement("w:abstractNum")
    abstract.set(qn("w:abstractNumId"), str(abstract_id))
    level = OxmlElement("w:lvl")
    level.set(qn("w:ilvl"), "0")
    start = OxmlElement("w:start")
    start.set(qn("w:val"), "1")
    kind = OxmlElement("w:numFmt")
    kind.set(qn("w:val"), "bullet")
    text = OxmlElement("w:lvlText")
    text.set(qn("w:val"), "\u2022")
    align = OxmlElement("w:lvlJc")
    align.set(qn("w:val"), "left")
    for element in (start, kind, text, align):
        level.append(element)
    abstract.append(level)
    numbering.insert(0, abstract)

    num = OxmlElement("w:num")
    num.set(qn("w:numId"), str(next_id))
    reference = OxmlElement("w:abstractNumId")
    reference.set(qn("w:val"), str(abstract_id))
    num.append(reference)
    numbering.append(num)
    return next_id


def _mark_list_item(paragraph, num_id: int) -> None:
    from docx.oxml import OxmlElement
    from docx.oxml.ns import qn

    if not num_id:
        return
    properties = paragraph._p.get_or_add_pPr()
    numbering = OxmlElement("w:numPr")
    level = OxmlElement("w:ilvl")
    level.set(qn("w:val"), "0")
    number = OxmlElement("w:numId")
    number.set(qn("w:val"), str(num_id))
    numbering.append(level)
    numbering.append(number)
    properties.append(numbering)


def _bottom_border(paragraph) -> None:
    """The PDF's section rule, drawn as the heading's bottom border."""
    from docx.oxml import OxmlElement
    from docx.oxml.ns import qn

    properties = paragraph._p.get_or_add_pPr()
    borders = OxmlElement("w:pBdr")
    bottom = OxmlElement("w:bottom")
    bottom.set(qn("w:val"), "single")
    bottom.set(qn("w:sz"), "4")
    bottom.set(qn("w:space"), "1")
    bottom.set(qn("w:color"), BLACK)
    borders.append(bottom)
    properties.append(borders)


def _add_runs(paragraph, runs: list[reflow.Run], document) -> None:
    from docx.shared import Pt

    index = 0
    while index < len(runs):
        run = runs[index]
        if run.link and run.text:
            group = [run]
            index += 1
            while index < len(runs) and runs[index].link == run.link and runs[index].text:
                group.append(runs[index])
                index += 1
            _hyperlink(paragraph, run.link, group)
            continue
        if run.image:
            try:
                width = min(72.0, max(4.0, run.image_width or 10.0))
                paragraph.add_run().add_picture(io.BytesIO(run.image), width=Pt(width))
            except Exception:  # noqa: BLE001 - a bad icon must not lose the document
                pass
            index += 1
            continue
        if not run.text:
            index += 1
            continue
        text_run = paragraph.add_run(run.text)
        if run.size:
            text_run.font.size = Pt(min(24.0, max(6.0, run.size)))
        if run.bold:
            text_run.font.bold = True
        if run.italic:
            text_run.font.italic = True
        if run.small_caps:
            text_run.font.small_caps = True
        index += 1


def _right_tab(paragraph, width_pt: float) -> None:
    from docx.enum.text import WD_TAB_ALIGNMENT
    from docx.shared import Pt

    paragraph.paragraph_format.tab_stops.add_tab_stop(
        Pt(width_pt), WD_TAB_ALIGNMENT.RIGHT
    )


def _table_shell(table, width_pt: float, columns: int, indent_pt: float = 0.0) -> None:
    """No borders, no cell padding, an explicit width and indent."""
    from docx.oxml import OxmlElement
    from docx.oxml.ns import qn
    from docx.shared import Pt

    table.autofit = False
    properties = table._tbl.tblPr

    def before(element, tag: str) -> None:
        reference = properties.find(qn(tag))
        if reference is not None:
            reference.addprevious(element)
        else:
            properties.append(element)

    # python-docx already created a `w:tblW` (auto/0). Appending a second one is
    # duplicate schema content and Word can keep the auto width instead of ours.
    width = properties.find(qn("w:tblW"))
    if width is None:
        width = OxmlElement("w:tblW")
        before(width, "w:tblLayout")
    width.set(qn("w:w"), str(int(Pt(width_pt).twips)))
    width.set(qn("w:type"), "dxa")

    existing_indent = properties.find(qn("w:tblInd"))
    if existing_indent is not None:
        properties.remove(existing_indent)
    indent = OxmlElement("w:tblInd")
    indent.set(qn("w:w"), str(int(Pt(indent_pt).twips)))
    indent.set(qn("w:type"), "dxa")
    before(indent, "w:tblLayout")

    existing_borders = properties.find(qn("w:tblBorders"))
    if existing_borders is not None:
        properties.remove(existing_borders)
    borders = OxmlElement("w:tblBorders")
    for side in ("top", "left", "bottom", "right", "insideH", "insideV"):
        edge = OxmlElement(f"w:{side}")
        edge.set(qn("w:val"), "none")
        edge.set(qn("w:sz"), "0")
        edge.set(qn("w:space"), "0")
        edge.set(qn("w:color"), "auto")
        borders.append(edge)
    before(borders, "w:tblLayout")

    existing_margins = properties.find(qn("w:tblCellMar"))
    if existing_margins is not None:
        properties.remove(existing_margins)
    margins = OxmlElement("w:tblCellMar")
    for side in ("top", "left", "bottom", "right"):
        edge = OxmlElement(f"w:{side}")
        edge.set(qn("w:w"), "0")
        edge.set(qn("w:type"), "dxa")
        margins.append(edge)
    before(margins, "w:tblLook")

    for column in table.columns:
        column.width = Pt(width_pt / max(1, columns))


def _write_header(document, header: layout.Header) -> None:
    from docx.enum.table import WD_CELL_VERTICAL_ALIGNMENT
    from docx.shared import Pt

    left_margin = document.sections[0].left_margin.pt
    content_width = (
        document.sections[0].page_width.pt
        - document.sections[0].left_margin.pt
        - document.sections[0].right_margin.pt
    )

    if header.photo is not None and header.title:
        gap = 12.0
        photo_cell = max(header.photo.width + gap, 30.0)
        indent = max(0.0, header.x - left_margin)
        table_width = max(photo_cell + 120.0, content_width - indent)
        table = document.add_table(rows=1, cols=2)
        _table_shell(
            table,
            table_width,
            2,
            indent_pt=indent,
        )
        left, right = table.rows[0].cells
        left.width = Pt(photo_cell)
        right.width = Pt(table_width - photo_cell)
        left.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.TOP
        right.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.TOP
        picture_paragraph = left.paragraphs[0]
        picture_paragraph.paragraph_format.space_before = Pt(0)
        picture_paragraph.paragraph_format.space_after = Pt(0)
        try:
            picture_paragraph.add_run().add_picture(
                io.BytesIO(header.photo.data), width=Pt(header.photo.width)
            )
        except Exception:  # noqa: BLE001 - a bad photo must not lose the document
            pass
        paragraph = right.paragraphs[0]
        paragraph.paragraph_format.space_before = Pt(0)
        paragraph.paragraph_format.space_after = Pt(0)
        paragraph.paragraph_format.line_spacing = 1.0
        _add_runs(paragraph, header.title, document)
        for address in header.address:
            paragraph = right.add_paragraph()
            paragraph.paragraph_format.space_before = Pt(0)
            paragraph.paragraph_format.space_after = Pt(0)
            paragraph.paragraph_format.line_spacing = 1.0
            _add_runs(paragraph, address, document)
    else:
        if header.photo is not None:
            paragraph = document.add_paragraph()
            paragraph.paragraph_format.left_indent = Pt(max(0.0, header.x - left_margin))
            paragraph.paragraph_format.space_before = Pt(0)
            paragraph.paragraph_format.space_after = Pt(0)
            try:
                paragraph.add_run().add_picture(
                    io.BytesIO(header.photo.data), width=Pt(header.photo.width)
                )
            except Exception:  # noqa: BLE001 - a bad photo must not lose the document
                pass
        if header.title:
            paragraph = document.add_paragraph()
            paragraph.paragraph_format.left_indent = Pt(max(0.0, header.x - left_margin))
            paragraph.paragraph_format.space_before = Pt(0)
            paragraph.paragraph_format.space_after = Pt(0)
            _add_runs(paragraph, header.title, document)
        for address in header.address:
            paragraph = document.add_paragraph()
            paragraph.paragraph_format.left_indent = Pt(max(0.0, header.x - left_margin))
            paragraph.paragraph_format.space_before = Pt(0)
            paragraph.paragraph_format.space_after = Pt(0)
            paragraph.paragraph_format.line_spacing = 1.0
            _add_runs(paragraph, address, document)

    if header.contact:
        from docx.enum.text import WD_TAB_ALIGNMENT

        paragraph = document.add_paragraph()
        paragraph.paragraph_format.space_before = Pt(0)
        # The source leaves a gap between the contact line and the first section
        # heading; a zero-spaced flow put "Summary" directly under the links.
        paragraph.paragraph_format.space_after = Pt(10)
        paragraph.paragraph_format.line_spacing = 1.0
        start = header.contact_x[0]
        paragraph.paragraph_format.left_indent = Pt(max(0.0, start - left_margin))
        for offset in header.contact_x[1:]:
            paragraph.paragraph_format.tab_stops.add_tab_stop(
                Pt(max(0.0, offset - start)), WD_TAB_ALIGNMENT.LEFT
            )
        _add_runs(paragraph, header.contact[0], document)
        for segment in header.contact[1:]:
            paragraph.add_run("\t")
            _add_runs(paragraph, segment, document)


def _write_grid(document, block: layout.Block, list_number: int, content_width: float) -> None:
    from docx.enum.table import WD_CELL_VERTICAL_ALIGNMENT
    from docx.shared import Pt

    columns = max(len(row) for row in block.cells)
    table = document.add_table(rows=len(block.cells), cols=columns)
    indent = 10.8
    _table_shell(table, content_width - 2 * indent, columns, indent_pt=indent)
    for row, cells in enumerate(block.cells):
        for column in range(columns):
            cell = table.cell(row, column)
            cell.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.TOP
            cell.width = Pt((content_width - 2 * indent) / columns)
            paragraph = cell.paragraphs[0]
            paragraph.style = document.styles["List Bullet"]
            paragraph.paragraph_format.left_indent = Pt(27)
            paragraph.paragraph_format.first_line_indent = Pt(-9)
            paragraph.paragraph_format.space_before = Pt(0)
            paragraph.paragraph_format.space_after = Pt(0)
            paragraph.paragraph_format.line_spacing = 1.0
            runs = cells[column] if column < len(cells) else []
            _add_runs(paragraph, runs, document)
            _mark_list_item(paragraph, list_number)


def _write_block(
    document,
    block: layout.Block,
    list_number: int,
    content_width: float,
) -> None:
    from docx.shared import Pt

    if block.kind == "heading":
        style = "Heading 1" if block.level <= 1 else f"Heading {block.level}"
        try:
            paragraph = document.add_paragraph(style=style)
        except KeyError:
            paragraph = document.add_paragraph()
        _add_runs(paragraph, block.runs, document)
        if block.border:
            _bottom_border(paragraph)
        return

    if block.kind == "grid":
        _write_grid(document, block, list_number, content_width)
        return

    if block.kind == "picture" and block.picture is not None:
        paragraph = document.add_paragraph()
        try:
            paragraph.add_run().add_picture(
                io.BytesIO(block.picture.data),
                width=Pt(min(content_width, max(12.0, block.picture.width))),
            )
        except Exception:  # noqa: BLE001 - a bad picture must not lose the document
            pass
        return

    if block.kind in ("entry", "detail", "project"):
        style = "Heading 2" if block.kind == "entry" else "Body Text"
        try:
            paragraph = document.add_paragraph(style=style)
        except KeyError:
            paragraph = document.add_paragraph()
        _add_runs(paragraph, block.left, document)
        _right_tab(paragraph, content_width - 10.8)
        paragraph.add_run("\t")
        _add_runs(paragraph, block.right, document)
        return

    if block.kind == "bullet":
        try:
            paragraph = document.add_paragraph(style="List Bullet")
        except KeyError:
            paragraph = document.add_paragraph()
        _add_runs(paragraph, block.runs, document)
        _mark_list_item(paragraph, list_number)
        return

    if block.kind == "skill":
        try:
            paragraph = document.add_paragraph(style="Body Text")
        except KeyError:
            paragraph = document.add_paragraph()
        paragraph.paragraph_format.left_indent = Pt(10.8)
        _add_runs(paragraph, block.runs, document)
        return

    try:
        paragraph = document.add_paragraph(style="Body Text")
    except KeyError:
        paragraph = document.add_paragraph()
    _add_runs(paragraph, block.runs, document)


def _configure(document, page) -> tuple[float, float]:
    """Letter/A4 page from the PDF, compact styles, and the content width."""
    from docx.enum.text import WD_ALIGN_PARAGRAPH
    from docx.shared import Pt, RGBColor

    section = document.sections[0]
    section.page_width = Pt(page.rect.width)
    section.page_height = Pt(page.rect.height)
    # The reference PDF-to-Word result uses a quarter-inch side margin; keeping
    # the same body width is what lets a dense one-page résumé stay one page.
    section.left_margin = Pt(18)
    section.right_margin = Pt(18)
    section.top_margin = Pt(16)
    section.bottom_margin = Pt(14)
    section.header_distance = Pt(0)
    section.footer_distance = Pt(0)

    normal = document.styles["Normal"]
    normal.font.name = "Calibri"
    normal.font.size = Pt(BODY_PT)
    normal.font.color.rgb = RGBColor(0, 0, 0)
    normal.paragraph_format.space_before = Pt(0)
    normal.paragraph_format.space_after = Pt(0)
    normal.paragraph_format.line_spacing = 1.0
    normal.paragraph_format.alignment = WD_ALIGN_PARAGRAPH.LEFT

    for name, size, before, after in (
        ("Heading 1", HEADING_1_PT, 4, 0),
        ("Heading 2", HEADING_2_PT, 2, 0),
        ("Heading 3", BODY_PT, 1, 0),
    ):
        style = document.styles[name]
        style.font.name = "Calibri"
        style.font.size = Pt(size)
        style.font.bold = True
        style.font.color.rgb = RGBColor(0, 0, 0)
        style.paragraph_format.space_before = Pt(before)
        style.paragraph_format.space_after = Pt(after)
        style.paragraph_format.line_spacing = 1.0
        style.paragraph_format.left_indent = Pt(10.8)
        style.paragraph_format.right_indent = Pt(10.8)
        style.paragraph_format.keep_with_next = True

    for name in ("Body Text", "List Bullet"):
        style = document.styles[name]
        style.font.name = "Calibri"
        style.font.size = Pt(BODY_PT)
        style.font.color.rgb = RGBColor(0, 0, 0)
        style.paragraph_format.space_before = Pt(0)
        style.paragraph_format.space_after = Pt(0)
        style.paragraph_format.line_spacing = 1.0
        style.paragraph_format.left_indent = Pt(10.8)
        style.paragraph_format.right_indent = Pt(10.8)
        style.paragraph_format.first_line_indent = Pt(0)

    bullet = document.styles["List Bullet"]
    bullet.paragraph_format.left_indent = Pt(25.2)
    bullet.paragraph_format.first_line_indent = Pt(-9)

    content_width = (
        section.page_width.pt - section.left_margin.pt - section.right_margin.pt
    )
    return content_width, section.left_margin.pt


def convert(source: str, target: str) -> None:
    import pymupdf as fitz
    from docx import Document
    from docx.shared import Pt

    with fitz.open(source) as pdf:
        if pdf.page_count == 0:
            raise ValueError("the PDF has no pages")
        document = Document()
        content_width, _left_margin = _configure(document, pdf[0])
        list_number = _list_numbering(document)
        layout_data = reflow.measure(pdf)

        for page_index, page in enumerate(pdf):
            if page_index > 0:
                document.add_page_break()
            header, blocks, _rows = layout.frame(page, layout_data)
            # Running headers and page numbers are part of the page, not only of
            # page 1. A repeated letterhead is preserved too.
            _write_header(document, header)
            for block in blocks:
                _write_block(document, block, list_number, content_width)

        metadata = pdf.metadata or {}
        title = metadata.get("title") or ""
        author = metadata.get("author") or ""
        subject = metadata.get("subject") or ""
        if title:
            document.core_properties.title = title
        if author:
            document.core_properties.author = author
        if subject:
            document.core_properties.subject = subject

        document.save(target)

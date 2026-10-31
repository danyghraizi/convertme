"""PDF → DOCX: the reflow analyser decides the structure, this writes it.

No layout library and no page breaks: the document is a flowing Word file whose
headings, list items, inline pictures and hyperlinks are the ones the PDF actually
had. A page break per PDF page turned a one-page résumé into a two-page Word file
with an empty second page, which is what the previous version did.
"""

from __future__ import annotations

import io

from . import reflow


def _hyperlink(paragraph, url: str, run_specs: list[reflow.Run]) -> None:
    """A real `w:hyperlink` run, not blue text that happens to look like a link."""
    from docx.oxml.ns import qn
    from docx.oxml import OxmlElement
    from docx.shared import Pt
    from docx.text.run import Run as DocxRun

    part = paragraph.part
    relationship = part.relate_to(
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
        run.font.color.rgb = None


def _new_run_element(parent):
    from docx.oxml import OxmlElement

    element = OxmlElement("w:r")
    parent.append(element)
    return element


def _list_numbering(document) -> int:
    """A bullet `numId` this document owns, so list items carry real numbering.

    A `List Bullet` paragraph style alone renders bullets, but the file then has no
    `w:numPr` at all — a reader (and any downstream tool) cannot tell a list item from
    a paragraph that happens to look like one. One `w:num` is created here and every
    list paragraph points at it.
    """
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


def _add_runs(paragraph, runs: list[reflow.Run], document) -> None:
    from docx.shared import Inches, Pt

    index = 0
    while index < len(runs):
        run = runs[index]
        if run.link:
            # Consecutive runs sharing a link become one hyperlink.
            group = [run]
            index += 1
            while index < len(runs) and runs[index].link == run.link:
                group.append(runs[index])
                index += 1
            _hyperlink(paragraph, run.link, group)
            continue
        if run.image:
            try:
                width_pt = 12.0
                document.add_picture(
                    io.BytesIO(run.image), width=Inches(min(6.0, max(0.12, width_pt / 72.0)))
                )
            except Exception:  # noqa: BLE001 - a bad icon must not lose the document
                pass
            index += 1
            continue
        text_run = paragraph.add_run(run.text)
        if run.size:
            text_run.font.size = Pt(min(24.0, max(6.0, run.size)))
        text_run.font.bold = run.bold
        text_run.font.italic = run.italic
        index += 1


def convert(source: str, target: str) -> None:
    import pymupdf as fitz
    from docx import Document
    from docx.enum.text import WD_ALIGN_PARAGRAPH
    from docx.shared import Inches, Pt

    with fitz.open(source) as pdf:
        if pdf.page_count == 0:
            raise ValueError("the PDF has no pages")
        layout = reflow.measure(pdf)

        document = Document()
        style = document.styles["Normal"]
        style.font.name = layout.body_font or "Calibri"
        style.font.size = Pt(max(6.0, min(24.0, layout.body_size)))
        for section in document.sections:
            section.top_margin = Inches(1)
            section.bottom_margin = Inches(1)
            section.left_margin = Inches(1)
            section.right_margin = Inches(1)

        list_number = _list_numbering(document)

        for page in pdf:
            tables = reflow.acceptable_tables(page)
            table_rects = [fitz.Rect(0, 0, 0, 0) for _ in tables]
            spec = reflow.page_spec(page, layout, skip_rects=[])
            # Paragraphs and tables are written in the order they sit on the page: a
            # table at the top of a report must not land after the conclusion.
            items: list[tuple[float, int, object]] = [
                (paragraph.y, 0, paragraph) for paragraph in spec.paragraphs
            ]
            items.extend((table.y, 1, table) for table in tables)
            items.sort(key=lambda item: (item[0], item[1]))

            for _y, kind, payload in items:
                if kind == 1:
                    table = payload
                    columns = max(len(row) for row in table.rows)
                    word_table = document.add_table(rows=len(table.rows), cols=columns)
                    word_table.style = "Table Grid"
                    for r, row in enumerate(table.rows):
                        for c in range(columns):
                            value = row[c] if c < len(row) else ""
                            word_table.cell(r, c).text = "" if value is None else str(value)
                    continue
                paragraph_spec = payload
                if not paragraph_spec.runs:
                    continue
                if paragraph_spec.style == "Picture":
                    for run in paragraph_spec.runs:
                        if run.image:
                            try:
                                document.add_picture(io.BytesIO(run.image), width=Inches(2.0))
                            except Exception:  # noqa: BLE001
                                pass
                    continue
                try:
                    paragraph = document.add_paragraph(style=paragraph_spec.style)
                except KeyError:
                    # An absent style (a stripped template) still gets its text.
                    paragraph = document.add_paragraph()
                _add_runs(paragraph, paragraph_spec.runs, document)
                if paragraph_spec.align == "center":
                    paragraph.alignment = WD_ALIGN_PARAGRAPH.CENTER
                if paragraph_spec.style == "List Bullet":
                    _mark_list_item(paragraph, list_number)

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

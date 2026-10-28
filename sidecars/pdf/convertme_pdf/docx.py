"""PDF → DOCX with PyMuPDF text/table extraction and python-docx writing.

Deliberately does not use a heavyweight PDF-reflow library: the conversion walks
the page in reading order, emits headings for the largest font on a page, real
Word tables for detected tables, and inline images for the page's pictures.
"""

from __future__ import annotations

import io


def _heading_threshold(sizes: list[float]) -> float:
    if not sizes:
        return 0.0
    largest = max(sizes)
    return largest * 0.98


def convert(source: str, target: str) -> None:
    import pymupdf as fitz
    from docx import Document
    from docx.shared import Pt

    document = Document()
    with fitz.open(source) as pdf:
        if pdf.page_count == 0:
            raise ValueError("the PDF has no pages")

        for page_number, page in enumerate(pdf, start=1):
            data = page.get_text("dict")
            sizes: list[float] = []
            for block in data.get("blocks", []):
                for line in block.get("lines", []):
                    for span in line.get("spans", []):
                        if span.get("text", "").strip():
                            sizes.append(float(span.get("size", 0.0)))
            heading_size = _heading_threshold(sizes)

            # Tables first: their cell text is also present in the text layer, so
            # collect the rectangles to skip those spans below.
            table_rects = []
            try:
                finder = page.find_tables()
                for table in finder.tables or []:
                    rows = table.extract()
                    if not rows:
                        continue
                    columns = max(len(row) for row in rows)
                    word_table = document.add_table(rows=len(rows), cols=columns)
                    word_table.style = "Table Grid"
                    for r, row in enumerate(rows):
                        for c in range(columns):
                            value = row[c] if c < len(row) else ""
                            word_table.cell(r, c).text = "" if value is None else str(value)
                    table_rects.append(fitz.Rect(table.bbox))
            except Exception:  # noqa: BLE001 - table detection is best effort
                table_rects = []

            for block in data.get("blocks", []):
                if block.get("type") == 1:  # image block
                    try:
                        image = fitz.Pixmap(pdf, block["number"])
                        if image.n - image.alpha >= 4:
                            image = fitz.Pixmap(fitz.csRGB, image)
                        document.add_picture(io.BytesIO(image.tobytes("png")))
                    except Exception:  # noqa: BLE001 - a broken image must not fail the export
                        pass
                    continue

                spans = [
                    span
                    for line in block.get("lines", [])
                    for span in line.get("spans", [])
                    if span.get("text", "").strip()
                ]
                if not spans:
                    continue
                if any(fitz.Rect(block["bbox"]).intersects(rect) for rect in table_rects):
                    continue

                text = " ".join(span["text"].strip() for span in spans)
                biggest = max(float(span.get("size", 0.0)) for span in spans)
                if heading_size > 0 and biggest >= heading_size and len(text) < 120:
                    document.add_heading(text, level=1 if page_number == 1 else 2)
                else:
                    paragraph = document.add_paragraph(text)
                    for run in paragraph.runs:
                        run.font.size = Pt(max(6.0, min(24.0, biggest)))

            if page_number < pdf.page_count:
                document.add_page_break()

    document.save(target)

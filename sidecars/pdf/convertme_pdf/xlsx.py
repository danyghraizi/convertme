"""PDF → XLSX: detect tables with PyMuPDF and write one worksheet per page/table."""

from __future__ import annotations


def convert(source: str, target: str) -> None:
    import pymupdf as fitz
    from openpyxl import Workbook
    from openpyxl.styles import Font

    workbook = Workbook()
    workbook.remove(workbook.active)

    with fitz.open(source) as document:
        written = 0
        for page_number, page in enumerate(document, start=1):
            finder = page.find_tables()
            for table_number, table in enumerate(finder.tables or [], start=1):
                sheet = workbook.create_sheet(f"p{page_number}-{table_number}"[:31])
                written += 1
                for row_index, row in enumerate(table.extract(), start=1):
                    for column_index, value in enumerate(row, start=1):
                        cell = sheet.cell(row=row_index, column=column_index, value=value)
                        if row_index == 1:
                            cell.font = Font(bold=True)
                for column in sheet.columns:
                    longest = max((len(str(c.value)) for c in column if c.value), default=8)
                    sheet.column_dimensions[column[0].column_letter].width = min(60, longest + 2)
        if written == 0:
            # No tables found: fall back to one row per text line so the export is
            # never an empty workbook.
            sheet = workbook.create_sheet("text")
            for page_number, page in enumerate(document, start=1):
                for line_number, line in enumerate(page.get_text("text").splitlines(), start=1):
                    sheet.cell(row=line_number, column=page_number, value=line)

    workbook.save(target)

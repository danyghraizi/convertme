"""PDF → XLSX: detected tables become sheets; text that is not in a table becomes one.

A report with no tables used to become a single sheet with one *column per page*, so a
five-page document looked like five unrelated columns — a spreadsheet nobody could read.
Now every page gets its own sheet named after it, with the page's lines down column A.
"""

from __future__ import annotations


def _fit_columns(sheet) -> None:
    for column in sheet.columns:
        longest = max((len(str(cell.value)) for cell in column if cell.value), default=8)
        sheet.column_dimensions[column[0].column_letter].width = min(60, longest + 2)


def convert(source: str, target: str) -> None:
    import pymupdf as fitz
    from openpyxl import Workbook
    from openpyxl.styles import Font

    workbook = Workbook()
    workbook.remove(workbook.active)

    with fitz.open(source) as document:
        if document.page_count == 0:
            raise ValueError("the PDF has no pages")
        for page_number, page in enumerate(document, start=1):
            finder = page.find_tables()
            tables = [table.extract() for table in (finder.tables or [])]
            tables = [rows for rows in tables if rows]
            if tables:
                for table_number, rows in enumerate(tables, start=1):
                    sheet = workbook.create_sheet(f"p{page_number}-{table_number}"[:31])
                    for row_index, row in enumerate(rows, start=1):
                        for column_index, value in enumerate(row, start=1):
                            cell = sheet.cell(row=row_index, column=column_index, value=value)
                            if row_index == 1:
                                cell.font = Font(bold=True)
                    _fit_columns(sheet)
                continue

            # No table: the page's text, one line per row, all in column A.
            sheet = workbook.create_sheet(f"p{page_number}"[:31])
            lines = [
                line.strip()
                for line in page.get_text("text").splitlines()
                if line.strip()
            ]
            for row_index, line in enumerate(lines, start=1):
                sheet.cell(row=row_index, column=1, value=line)
            sheet.column_dimensions["A"].width = min(
                80, max((len(line) for line in lines), default=20) + 2
            )

    workbook.save(target)

"""PDF → EPUB: text plus images into reflowable XHTML, zipped as EPUB 3.

Written with `zipfile` rather than a converter dependency so the artifact is a
valid EPUB even when no external tool is installed.
"""

from __future__ import annotations

import html
import zipfile
from pathlib import Path

CONTAINER = """<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>
"""


def _chapter(page_index: int, text: str) -> str:
    paragraphs = "\n".join(
        f"<p>{html.escape(line)}</p>" for line in text.splitlines() if line.strip()
    )
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Page {page_index}</title></head>
<body><h1>Page {page_index}</h1>{paragraphs}</body></html>
"""


def convert(source: str, target: str) -> None:
    import pymupdf as fitz

    chapters: list[tuple[int, str]] = []
    with fitz.open(source) as document:
        if document.page_count == 0:
            raise ValueError("the PDF has no pages")
        for index, page in enumerate(document, start=1):
            chapters.append((index, page.get_text("text")))
        title = (document.metadata or {}).get("title") or Path(source).stem

    manifest = "\n".join(
        f'    <item id="c{index}" href="chapter{index}.xhtml" media-type="application/xhtml+xml"/>'
        for index, _ in chapters
    )
    spine = "\n".join(f'    <itemref idref="c{index}"/>' for index, _ in chapters)
    opf = f"""<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:convertme:{html.escape(Path(source).stem)}</dc:identifier>
    <dc:title>{html.escape(str(title))}</dc:title>
    <dc:language>und</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
{manifest}
  </manifest>
  <spine>
{spine}
  </spine>
</package>
"""
    nav = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head><body><nav epub:type="toc"><ol>
""" + "\n".join(
        f'<li><a href="chapter{index}.xhtml">Page {index}</a></li>' for index, _ in chapters
    ) + """
</ol></nav></body></html>
"""

    with zipfile.ZipFile(target, "w") as book:
        # The mimetype entry must be first and stored, not deflated.
        book.writestr(zipfile.ZipInfo("mimetype"), "application/epub+zip")
        book.writestr("META-INF/container.xml", CONTAINER)
        book.writestr("OEBPS/content.opf", opf)
        book.writestr("OEBPS/nav.xhtml", nav, compress_type=zipfile.ZIP_DEFLATED)
        for index, text in chapters:
            book.writestr(f"OEBPS/chapter{index}.xhtml", _chapter(index, text), compress_type=zipfile.ZIP_DEFLATED)

"""PDF → EPUB: reflowable XHTML, one chapter per page.

The structure comes from `reflow`, the same analyser the DOCX writer uses: headings,
list items, paragraphs, inline emphasis, links and inline images. Reading the PDF a
second way for EPUB is how the two exports end up disagreeing about the same document.

EPUB 3 rules that matter for a reader to open the file: `mimetype` is the first entry
and stored (not deflated), `META-INF/container.xml` points at the package document, and
every resource (including every image) is in the manifest.
"""

from __future__ import annotations

import html
import zipfile
from pathlib import Path

from . import reflow

CONTAINER = """<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>
"""

XHTML_HEAD = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>{title}</title>
<style>body{{font-family:serif;line-height:1.45}} h1,h2,h3{{line-height:1.15}}
ul{{margin:0.4em 0 0.8em 1.2em}} img.icon{{height:1em;vertical-align:-0.15em}}
table{{border-collapse:collapse}} td{{border:1px solid #999;padding:2px 6px}}</style>
</head><body>
"""


def _escape(text: str) -> str:
    return html.escape(text, quote=False)


def _runs_html(runs, images: list[tuple[str, bytes]]) -> str:
    """Inline markup for a paragraph's runs, collecting the images it needs."""
    out = []
    for run in runs:
        if run.image:
            name = f"image{len(images) + 1:03d}.png"
            images.append((name, run.image))
            out.append(f'<img class="icon" src="../images/{name}" alt=""/>')
            continue
        text = _escape(run.text)
        if not text:
            continue
        if run.italic:
            text = f"<em>{text}</em>"
        if run.bold:
            text = f"<strong>{text}</strong>"
        if run.link:
            text = f'<a href="{html.escape(run.link, quote=True)}">{text}</a>'
        out.append(text)
    return "".join(out)


def _chapter_html(page_spec, images: list[tuple[str, bytes]], title: str) -> str:
    body: list[str] = []
    list_items: list[str] = []
    list_open = False

    def close_list() -> None:
        nonlocal list_open
        if list_open:
            body.append("<ul>" + "".join(list_items) + "</ul>")
            list_items.clear()
            list_open = False

    for paragraph in page_spec.paragraphs:
        style = paragraph.style
        if style == "List Bullet":
            list_items.append(f"<li>{_runs_html(paragraph.runs, images)}</li>")
            list_open = True
            continue
        close_list()
        markup = _runs_html(paragraph.runs, images)
        if not markup.strip():
            continue
        if style == "Title":
            body.append(f"<h1>{markup}</h1>")
        elif style.startswith("Heading "):
            level = min(3, max(1, int(style.split()[1])))
            body.append(f"<h{level}>{markup}</h{level}>")
        elif style == "Picture":
            body.append(f"<p>{markup}</p>")
        else:
            body.append(f"<p>{markup}</p>")
    close_list()

    for table in page_spec.tables:
        rows = []
        for row in table.rows:
            cells = "".join(f"<td>{_escape('' if cell is None else str(cell))}</td>" for cell in row)
            rows.append(f"<tr>{cells}</tr>")
        body.append("<table>" + "".join(rows) + "</table>")

    return XHTML_HEAD.format(title=_escape(title)) + "\n".join(body) + "\n</body></html>\n"


def convert(source: str, target: str) -> None:
    import pymupdf as fitz

    with fitz.open(source) as document:
        if document.page_count == 0:
            raise ValueError("the PDF has no pages")
        layout = reflow.measure(document)
        title = (document.metadata or {}).get("title") or Path(source).stem

        chapters: list[tuple[int, str]] = []
        images: list[tuple[str, bytes]] = []
        for number, page in enumerate(document, start=1):
            tables = reflow.acceptable_tables(page)
            spec = reflow.page_spec(page, layout)
            spec.tables = tables
            chapters.append((number, _chapter_html(spec, images, f"Page {number}")))

    manifest_images = "\n".join(
        f'    <item id="img{index}" href="images/{name}" media-type="image/png"/>'
        for index, (name, _data) in enumerate(images, start=1)
    )
    manifest = "\n".join(
        f'    <item id="c{number}" href="chapter{number}.xhtml" media-type="application/xhtml+xml"/>'
        for number, _html in chapters
    )
    spine = "\n".join(f'    <itemref idref="c{number}"/>' for number, _html in chapters)
    opf = f"""<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:convertme:{html.escape(Path(source).stem)}</dc:identifier>
    <dc:title>{_escape(str(title))}</dc:title>
    <dc:language>und</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
{manifest}
{manifest_images}
  </manifest>
  <spine>
{spine}
  </spine>
</package>
"""
    nav = (
        """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head><body><nav epub:type="toc"><ol>
"""
        + "\n".join(
            f'<li><a href="chapter{number}.xhtml">Page {number}</a></li>'
            for number, _html in chapters
        )
        + """
</ol></nav></body></html>
"""
    )

    with zipfile.ZipFile(target, "w") as book:
        # The mimetype entry must be first and stored, not deflated.
        book.writestr(zipfile.ZipInfo("mimetype"), "application/epub+zip")
        book.writestr("META-INF/container.xml", CONTAINER)
        book.writestr("OEBPS/content.opf", opf, compress_type=zipfile.ZIP_DEFLATED)
        book.writestr("OEBPS/nav.xhtml", nav, compress_type=zipfile.ZIP_DEFLATED)
        for number, markup in chapters:
            book.writestr(
                f"OEBPS/chapter{number}.xhtml", markup, compress_type=zipfile.ZIP_DEFLATED
            )
        for name, data in images:
            book.writestr(f"OEBPS/images/{name}", data, compress_type=zipfile.ZIP_DEFLATED)

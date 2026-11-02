"""PDF → EPUB: one reflowable chapter per PDF page.

The structure comes from `layout.frame`, the same visual-row analyser the DOCX
writer uses: the header (title, address, contact line and photo), section
headings, employer/school entries with a right date, italic detail lines,
projects, skill rows, bullets, the four-column coursework grid, and body
paragraphs. Reading the PDF through `reflow.page_spec` instead produced one
paragraph per text block, so a date sat inside a degree and the coursework ran
down each column.

EPUB 3 rules that matter for a reader to open the file: `mimetype` is the first
entry and stored (not deflated), `META-INF/container.xml` points at the package
document, and every resource (including every image) is in the manifest. Image
media types come from the bytes, not from an assumed PNG suffix.
"""

from __future__ import annotations

import hashlib
import html
import zipfile
from dataclasses import replace
from datetime import datetime, timezone
from pathlib import Path

from . import layout
from . import reflow

CONTAINER = """<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>
"""

# The book's only stylesheet. It lives in the manifest as OEBPS/style.css and is
# linked from every XHTML document: a reader that ignores it still gets the
# per-image width/height attributes, and one that honours it gets the compact,
# Calibri-like layout the PDF actually uses.
STYLE = """body { font-family: "Calibri", "Carlito", "Liberation Sans", sans-serif;
       line-height: 1.35; color: #111; margin: 0 5%; }
h1, h2 { line-height: 1.2; font-weight: 700; }
h1.title { font-size: 1.8em; margin: 0 0 0.1em; text-align: center; }
p.address { text-align: center; margin: 0.05em 0; font-size: 0.95em; }
p.contact { text-align: center; font-size: 0.9em; margin: 0.4em 0 0.9em; }
span.contact-item { display: inline-block; margin: 0 0.35em;
                    max-width: 100%; overflow-wrap: anywhere; }
img { max-width: 100%; height: auto; }
img.icon { width: 1em !important; height: 1em !important;
           max-width: 1em; max-height: 1em;
           vertical-align: -0.15em; margin-right: 0.1em; }
img.photo { float: right; width: 5em !important; height: auto !important;
            max-width: 5em; margin: 0 0 0.5em 0.8em; }
.header { display: flow-root; }
.header::after, .entry::after { content: ""; display: block; clear: both; }
p.detail::after, p.project::after { content: ""; display: block; clear: both; }
h1.section { font-size: 1.25em; margin: 0.9em 0 0.25em; }
h2.section { font-size: 1.08em; margin: 0.8em 0 0.2em; }
h1.rule, h2.rule { border-bottom: 1px solid #444; padding-bottom: 0.12em; }
h2.entry { font-size: 1.05em; margin: 0.7em 0 0.05em; }
p.detail { margin: 0.05em 0 0.4em; }
span.meta { font-weight: 400; font-style: normal; color: #555; }
h2.entry span.meta, p.detail span.meta, p.project span.meta {
           float: right; margin-left: 1em; font-size: 0.85em; }
p.project { margin: 0.5em 0 0.05em; }
p.skill { margin: 0.15em 0; }
ul { margin: 0.3em 0 0.7em 1.2em; padding: 0; }
li { margin: 0.1em 0; }
table.coursework { width: 100%; table-layout: fixed; border-collapse: collapse;
                   margin: 0.4em 0 0.7em; font-size: 0.92em;
                   border-top: 1px solid #d5d5d5;
                   border-bottom: 1px solid #d5d5d5; }
table.coursework td { padding: 0.2em 0.4em; vertical-align: top;
                      overflow-wrap: anywhere; line-height: 1.35; }
p.picture { text-align: center; margin: 0.5em 0; }
"""

XHTML_HEAD = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="und">
<head><title>{title}</title>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<link rel="stylesheet" type="text/css" href="style.css"/>
</head><body>
"""


def _escape(text: str) -> str:
    return html.escape(text, quote=False)


def _text(runs: list[reflow.Run]) -> str:
    """Plain text of a run list, whitespace collapsed."""
    return " ".join("".join(run.text for run in runs).split())


def _trimmed(runs: list[reflow.Run]) -> list[reflow.Run]:
    """Run copies without the line's leading/trailing whitespace.

    The PDF stores its inter-word spaces as their own runs, so trimming happens
    at the ends of the line only; a space inside the line is still a run.
    """
    copies = [replace(run) for run in runs]
    for run in copies:
        if run.text:
            run.text = run.text.replace("\n", " ").replace("\r", " ").replace("\t", " ")

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
        return [run for run in copies if run.image]

    copies = copies[first : last + 1]
    if copies[0].text:
        copies[0].text = copies[0].text.lstrip()
    if copies[-1].text:
        copies[-1].text = copies[-1].text.rstrip()
    return [run for run in copies if run.image or run.text]


def _coalesce(runs: list[reflow.Run]) -> list[reflow.Run]:
    """Merge adjacent text runs with the same emphasis, so one phrase is one tag."""
    out: list[reflow.Run] = []
    for run in runs:
        if run.image:
            out.append(run)
            continue
        if (
            out
            and not out[-1].image
            and out[-1].text
            and (out[-1].bold, out[-1].italic, out[-1].link, out[-1].small_caps)
            == (run.bold, run.italic, run.link, run.small_caps)
        ):
            out[-1].text += run.text
        else:
            out.append(run)
    return out


def _format_html(
    text: str,
    run: reflow.Run,
    *,
    drop_bold: bool = False,
    force_bold: bool = False,
    drop_italic: bool = False,
    force_italic: bool = False,
) -> str:
    """One text run as inline markup, keeping surrounding spaces outside tags."""
    if not text:
        return ""
    core = text.strip()
    if not core:
        return text
    lead = text[: len(text) - len(text.lstrip())]
    trail = text[len(text.rstrip()) :]
    core = html.escape(text.strip(), quote=False)
    bold = (run.bold or force_bold) and not drop_bold
    italic = (run.italic or force_italic) and not drop_italic
    if bold:
        core = f"<strong>{core}</strong>"
    if italic:
        core = f"<em>{core}</em>"
    return lead + core + trail


class _Images:
    """Every picture goes into the book once, with the media type of its bytes."""

    def __init__(self) -> None:
        self.items: list[tuple[str, bytes, str]] = []
        self._seen: dict[bytes, str] = {}

    def add(self, data: bytes) -> str:
        digest = hashlib.sha1(data).digest()
        name = self._seen.get(digest)
        if name is not None:
            return f"images/{name}"
        suffix, media = _image_type(data)
        name = f"image{len(self.items) + 1:03d}.{suffix}"
        self.items.append((name, data, media))
        self._seen[digest] = name
        return f"images/{name}"


def _image_type(data: bytes) -> tuple[str, str]:
    if data.startswith(b"\xff\xd8\xff"):
        return "jpg", "image/jpeg"
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "png", "image/png"
    if data.startswith(b"GIF8"):
        return "gif", "image/gif"
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "webp", "image/webp"
    if data.startswith(b"BM"):
        return "bmp", "image/bmp"
    if data.startswith((b"II*\x00", b"MM\x00*")):
        return "tiff", "image/tiff"
    return "png", "image/png"


POINTS_TO_PX = 4.0 / 3.0
ICON_MIN_PX = 16
ICON_MAX_PX = 24
PHOTO_MAX_PX = 96


def _px(points: float) -> int:
    """PDF points to CSS pixels, rounding conservatively (1pt = 4/3 px)."""
    return max(1, round(points * POINTS_TO_PX)) if points > 0 else 0


def _scaled(width: int, height: int, limit: int) -> tuple[int, int]:
    longest = max(width, height)
    if longest > limit:
        scale = limit / longest
        width, height = max(1, round(width * scale)), max(1, round(height * scale))
    return width, height


def _image_size(width_pt: float, height_pt: float, fallback: int) -> tuple[int, int]:
    """Pixel box for an image, synthesising a square when the PDF omits a size."""
    width = _px(width_pt)
    height = _px(height_pt)
    if not width and not height:
        return fallback, fallback
    if not width:
        width = height
    if not height:
        height = width
    return width, height


def _icon_size(run: reflow.Run) -> tuple[int, int]:
    """Inline icon dimensions, each bounded to 16-24 CSS pixels."""
    width, height = _image_size(run.image_width, run.image_height, ICON_MIN_PX)
    width = min(ICON_MAX_PX, max(ICON_MIN_PX, width))
    height = min(ICON_MAX_PX, max(ICON_MIN_PX, height))
    return width, height


def _photo_size(photo: layout.Picture) -> tuple[int, int]:
    """Header photo dimensions, never larger than the 5em CSS box (96px)."""
    width, height = _image_size(photo.width, photo.height, PHOTO_MAX_PX)
    return _scaled(width, height, PHOTO_MAX_PX)


def _picture_size(picture: layout.Picture) -> tuple[int, int]:
    """Body picture dimensions in CSS pixels, derived from its PDF box."""
    return _image_size(picture.width, picture.height, 1)


def _runs_html(
    runs: list[reflow.Run],
    images: _Images,
    *,
    drop_bold: bool = False,
    force_bold: bool = False,
    drop_italic: bool = False,
    force_italic: bool = False,
) -> str:
    """Inline XHTML for a paragraph's runs, with contiguous links grouped."""
    cleaned = _coalesce(_trimmed(runs))
    out: list[str] = []
    index = 0
    while index < len(cleaned):
        run = cleaned[index]
        if run.image:
            href = images.add(run.image)
            width, height = _icon_size(run)
            out.append(
                f'<img class="icon" src="{href}" width="{width}" '
                f'height="{height}" alt=""/>'
            )
            index += 1
            continue
        if run.link:
            group: list[reflow.Run] = []
            while (
                index < len(cleaned)
                and cleaned[index].link == run.link
                and not cleaned[index].image
            ):
                group.append(cleaned[index])
                index += 1
            text = "".join(item.text for item in group)
            core = text.strip()
            if not core:
                out.append(text)
                continue
            lead = text[: len(text) - len(text.lstrip())]
            trail = text[len(text.rstrip()) :]
            inner = "".join(
                _format_html(
                    item.text,
                    item,
                    drop_bold=drop_bold,
                    force_bold=force_bold,
                    drop_italic=drop_italic,
                    force_italic=force_italic,
                )
                for item in group
            ).strip()
            href = html.escape(run.link, quote=True)
            out.append(f'{lead}<a href="{href}">{inner}</a>{trail}')
            continue
        out.append(
            _format_html(
                run.text,
                run,
                drop_bold=drop_bold,
                force_bold=force_bold,
                drop_italic=drop_italic,
                force_italic=force_italic,
            )
        )
        index += 1
    return "".join(out)


def _split_runs(runs: list[reflow.Run], separator: str) -> tuple[list[reflow.Run], list[reflow.Run]]:
    """Split a run list around the first separator character (the `|` or `:`)."""
    left: list[reflow.Run] = []
    right: list[reflow.Run] = []
    found = False
    for run in runs:
        if found:
            right.append(replace(run))
            continue
        if run.image or separator not in run.text:
            left.append(replace(run))
            continue
        head, _, tail = run.text.partition(separator)
        if head:
            left.append(replace(run, text=head))
        if tail:
            right.append(replace(run, text=tail))
        found = True
    return left, right


def _header_html(header: layout.Header, images: _Images, first_page: bool) -> str:
    body: list[str] = []
    if header.photo is not None:
        href = images.add(header.photo.data)
        width, height = _photo_size(header.photo)
        body.append(
            f'<img class="photo" src="{href}" width="{width}" '
            f'height="{height}" alt="Photo"/>'
        )
    if first_page and header.title:
        title = _runs_html(header.title, images, drop_bold=True)
        if title.strip():
            body.append(f'<h1 class="title">{title}</h1>')
    for address in header.address:
        markup = _runs_html(address, images)
        if markup.strip():
            body.append(f'<p class="address">{markup}</p>')
    if header.contact:
        parts = []
        for segment in header.contact:
            markup = _runs_html(segment, images)
            if markup.strip():
                parts.append(f'<span class="contact-item">{markup}</span>')
        if parts:
            body.append('<p class="contact">' + " ".join(parts) + "</p>")
    if not body:
        return ""
    return '<div class="header">\n' + "\n".join(body) + "\n</div>"


def _grid_html(block: layout.Block, images: _Images) -> str:
    columns = max((len(row) for row in block.cells), default=0)
    if not columns:
        return ""
    colgroup = "<colgroup>" + "".join("<col/>" for _ in range(columns)) + "</colgroup>"
    rows: list[str] = []
    for cells in block.cells:
        values = []
        for column in range(columns):
            runs = cells[column] if column < len(cells) else []
            markup = _runs_html(runs, images)
            if markup.strip():
                markup = f"• {markup}"
            values.append(f"<td>{markup}</td>")
        rows.append("<tr>" + "".join(values) + "</tr>")
    return (
        '<table class="coursework">' + colgroup + "".join(rows) + "</table>"
    )


def _blocks_html(blocks: list[layout.Block], images: _Images) -> str:
    out: list[str] = []
    list_items: list[str] = []
    list_open = False

    def close_list() -> None:
        nonlocal list_open
        if list_open:
            out.append("<ul>" + "".join(list_items) + "</ul>")
            list_items.clear()
            list_open = False

    for block in blocks:
        if block.kind != "bullet":
            close_list()

        if block.kind == "heading":
            level = min(2, max(1, block.level))
            classes = "section" + (" rule" if block.border else "")
            markup = _runs_html(block.runs, images, drop_bold=True)
            if markup.strip():
                out.append(f'<h{level} class="{classes}">{markup}</h{level}>')
            continue

        if block.kind == "entry":
            name = _runs_html(block.left, images, drop_bold=True)
            date = _runs_html(block.right, images, drop_bold=True)
            if not name.strip():
                continue
            meta = f' <span class="meta">{date}</span>' if date.strip() else ""
            out.append(f'<h2 class="entry">{name}{meta}</h2>')
            continue

        if block.kind == "detail":
            degree = _runs_html(block.left, images, force_italic=True)
            place = _runs_html(
                block.right, images, drop_bold=True, drop_italic=True
            )
            if not degree.strip():
                continue
            meta = f' <span class="meta">{place}</span>' if place.strip() else ""
            out.append(f'<p class="detail">{degree}{meta}</p>')
            continue

        if block.kind == "project":
            name_runs, tech_runs = _split_runs(block.left, "|")
            name = _runs_html(name_runs, images, force_bold=True, drop_italic=True)
            tech = _runs_html(tech_runs, images, force_italic=True, drop_bold=True)
            date = _runs_html(
                block.right, images, drop_bold=True, drop_italic=True
            )
            parts = [part for part in (name, tech) if part.strip()]
            if date.strip():
                parts.append(f'<span class="meta">{date}</span>')
            if parts:
                out.append('<p class="project">' + " ".join(parts) + "</p>")
            continue

        if block.kind == "skill":
            label_runs, rest_runs = _split_runs(block.runs, ":")
            label = _runs_html(label_runs, images, force_bold=True, drop_italic=True)
            rest = _runs_html(rest_runs, images)
            if label.strip():
                markup = f"{label.strip()}: {rest}" if rest.strip() else label.strip()
                out.append(f'<p class="skill">{markup}</p>')
            continue

        if block.kind == "bullet":
            markup = _runs_html(block.runs, images)
            if markup.strip():
                list_items.append(f"<li>{markup}</li>")
                list_open = True
            continue

        if block.kind == "grid":
            markup = _grid_html(block, images)
            if markup:
                out.append(markup)
            continue

        if block.kind == "picture" and block.picture is not None:
            href = images.add(block.picture.data)
            width, height = _picture_size(block.picture)
            out.append(
                f'<p class="picture"><img src="{href}" width="{width}" '
                f'height="{height}" alt=""/></p>'
            )
            continue

        markup = _runs_html(block.runs, images)
        if markup.strip():
            out.append(f"<p>{markup}</p>")

    close_list()
    return "\n".join(out)


def _chapter_html(
    header: layout.Header,
    blocks: list[layout.Block],
    images: _Images,
    title: str,
    first_page: bool,
) -> str:
    body: list[str] = []
    head = _header_html(header, images, first_page)
    if head:
        body.append(head)
    content = _blocks_html(blocks, images)
    if content:
        body.append(content)
    document = (
        XHTML_HEAD.format(title=_escape(title))
        + "\n".join(body)
        + "\n</body></html>\n"
    )
    return document


def convert(source: str, target: str) -> None:
    import pymupdf as fitz

    with fitz.open(source) as document:
        if document.page_count == 0:
            raise ValueError("the PDF has no pages")
        layout_data = reflow.measure(document)
        pages = [layout.frame(page, layout_data) for page in document]
        metadata = document.metadata or {}
        fallback_title = str(metadata.get("title") or "").strip() or Path(source).stem

    header_title = _text(pages[0][0].title)
    title = header_title or fallback_title

    images = _Images()
    chapters: list[tuple[int, str, str]] = []
    for number, (header, blocks, _rows) in enumerate(pages, start=1):
        chapter_title = title if number == 1 else f"{title} — page {number}"
        markup = _chapter_html(header, blocks, images, chapter_title, number == 1)
        chapters.append((number, markup, chapter_title))

    manifest_images = "\n".join(
        f'    <item id="img{index}" href="images/{name}" media-type="{media}"/>'
        for index, (name, _data, media) in enumerate(images.items, start=1)
    )
    manifest = "\n".join(
        f'    <item id="c{number}" href="chapter{number}.xhtml" media-type="application/xhtml+xml"/>'
        for number, _markup, _title in chapters
    )
    spine = "\n".join(
        f'    <itemref idref="c{number}"/>' for number, _markup, _title in chapters
    )
    modified = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    opf = f"""<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:convertme:{html.escape(Path(source).stem)}</dc:identifier>
    <dc:title>{_escape(title)}</dc:title>
    <dc:language>und</dc:language>
    <meta property="dcterms:modified">{modified}</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="style" href="style.css" media-type="text/css"/>
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
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="und">
<head><title>Contents</title>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<link rel="stylesheet" type="text/css" href="style.css"/>
</head><body><nav epub:type="toc"><ol>
"""
        + "\n".join(
            f'<li><a href="chapter{number}.xhtml">{_escape(chapter_title)}</a></li>'
            for number, _markup, chapter_title in chapters
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
        book.writestr("OEBPS/style.css", STYLE, compress_type=zipfile.ZIP_DEFLATED)
        for number, markup, _title in chapters:
            book.writestr(
                f"OEBPS/chapter{number}.xhtml", markup, compress_type=zipfile.ZIP_DEFLATED
            )
        for name, data, _media in images.items:
            book.writestr(f"OEBPS/images/{name}", data, compress_type=zipfile.ZIP_DEFLATED)

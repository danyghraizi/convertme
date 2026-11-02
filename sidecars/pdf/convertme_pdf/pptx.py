"""PDF → PPTX with the page's own geometry, as editable shapes.

The old writer put a rendered page image on every slide: faithful, but nothing in the
deck could be edited. This one rebuilds the page from `layout.extract`, the same visual
rows the DOCX writer reads:

* the slide is exactly the page size (1 pt = 12700 EMU), so a 612×792 pt Letter page
  becomes a 7772400×10058400 EMU slide;
* every visual segment is its own text box at the PDF's coordinates — no wrapping and
  zero insets — so the right-hand dates keep their x, and the four coursework columns
  stay four columns;
* each run carries Calibri at the PDF's point size, its bold/italic, its hyperlink and
  small caps; the bullet glyph stays in the text, as the page has it;
* photographs, and icon glyphs rasterised by the reflow analyser, are placed as
  pictures at their boxes (an icon run splits its segment: picture, then text);
* the page's horizontal rules become thin connectors.

No text means a scan, which cannot be rebuilt from spans; that page falls back to a
render of the whole page so nothing is lost. A page turned by `/Rotate` keeps that
render too: its text is reported in the unrotated space, so every box would have to be
turned with it. A later page with a different aspect from the first is scaled to fit
the deck's canvas and centred.
"""

from __future__ import annotations

import io

from . import layout as layout_mod
from . import reflow

EMU_PER_PT = 12700
FONT = "Calibri"
TEXT_SLACK = 8.0  # points of extra box width; wrap is off, this only aids selection
RULE_PT = 0.75
RENDER_DPI = 200


def _emu(points: float) -> int:
    return int(round(points * EMU_PER_PT))


def _span_boxes(page) -> dict[tuple[float, float, float, float], list[tuple[tuple, str]]]:
    """Each line's span boxes, keyed by the line's bbox.

    `layout.Segment` keeps the runs but not where each one sits; an icon run needs its
    own x so its picture lands on the glyph, and the text after it starts past the
    picture. PyMuPDF reports the same line boxes here as in `layout._segment`.
    """
    boxes: dict[tuple[float, float, float, float], list[tuple[tuple, str]]] = {}
    try:
        data = page.get_text("dict", sort=True)
    except Exception:  # noqa: BLE001 - the caller falls back to the line's left edge
        return boxes
    for block in data.get("blocks", []):
        if block.get("type") == 1:
            continue
        for line in block.get("lines", []):
            key = tuple(round(float(value), 2) for value in line["bbox"])
            boxes[key] = [
                (tuple(float(value) for value in span["bbox"]), span.get("text", ""))
                for span in line.get("spans", [])
            ]
    return boxes


def _key(segment) -> tuple[float, float, float, float]:
    return (
        round(segment.x0, 2),
        round(segment.y0, 2),
        round(segment.x1, 2),
        round(segment.y1, 2),
    )


def _items(segment, boxes) -> list[tuple[reflow.Run, tuple]]:
    """Pair the segment's runs with their span boxes, in reading order.

    `_runs_for_line` emits one run per non-empty span, so the pairing is positional.
    A run whose line could not be matched keeps a box at the line's left edge.
    """
    items: list[tuple[reflow.Run, tuple]] = []
    index = 0
    for box, text in boxes:
        if not text:
            continue
        if index >= len(segment.runs):
            break
        items.append((segment.runs[index], box))
        index += 1

    cursor = segment.x0
    for run in segment.runs[index:]:
        width = run.image_width if run.image else segment.x1 - cursor
        if width <= 0:
            width = max(segment.x1 - cursor, 1.0)
        items.append((run, (cursor, segment.y0, cursor + width, segment.y1)))
        cursor += width
    return items


def _merge(runs: list[reflow.Run]) -> list[reflow.Run]:
    """Join adjacent text runs that share a format: the PDF splits every space."""
    merged: list[reflow.Run] = []
    for run in runs:
        if merged and run.image is None and merged[-1].image is None:
            previous = merged[-1]
            if (
                previous.bold == run.bold
                and previous.italic == run.italic
                and round(previous.size, 1) == round(run.size, 1)
                and previous.link == run.link
                and previous.small_caps == run.small_caps
            ):
                previous.text += run.text
                continue
        merged.append(reflow.Run(**run.__dict__))
    return merged


def _add_text(slide, runs, x0: float, segment, scale: float, ox: float, oy: float,
              body_size: float) -> None:
    """One unwrapped, zero-inset text box for a group of runs in one visual segment."""
    from pptx.enum.text import MSO_ANCHOR, MSO_AUTO_SIZE, PP_ALIGN
    from pptx.util import Emu, Pt

    runs = _merge(runs)
    if not "".join(run.text for run in runs).strip():
        return

    size = max((run.size for run in runs if run.text.strip()), default=0.0)
    size = size or segment.size or body_size or 10.0
    center = (segment.y0 + segment.y1) / 2
    height = max((segment.y1 - segment.y0) + 2.0, size * 1.35)
    top = center - height / 2
    width = max(segment.x1 - x0 + TEXT_SLACK, 4.0)

    box = slide.shapes.add_textbox(
        Emu(_emu(x0 * scale + ox)),
        Emu(_emu(top * scale + oy)),
        Emu(_emu(width * scale)),
        Emu(_emu(height * scale)),
    )
    frame = box.text_frame
    frame.word_wrap = False
    frame.margin_left = frame.margin_right = 0
    frame.margin_top = frame.margin_bottom = 0
    frame.auto_size = MSO_AUTO_SIZE.NONE
    frame.vertical_anchor = MSO_ANCHOR.MIDDLE
    paragraph = frame.paragraphs[0]
    paragraph.alignment = PP_ALIGN.LEFT
    paragraph.space_before = Pt(0)
    paragraph.space_after = Pt(0)
    paragraph.line_spacing = 1.0

    for run in runs:
        target = paragraph.add_run()
        target.text = run.text
        font = target.font
        font.name = FONT
        font.size = Pt(run.size or size)
        font.bold = bool(run.bold)
        font.italic = bool(run.italic)
        if run.small_caps:
            font._rPr.set("cap", "small")
        if run.link:
            # The PDF's links are black and underlined (hidelinks); an explicit colour
            # keeps PowerPoint from painting them with the theme's hyperlink hue.
            from pptx.dml.color import RGBColor

            font.color.rgb = RGBColor(0, 0, 0)
            font.underline = True
            target.hyperlink.address = run.link
        # A run can begin or end with the PDF's own spacing, which must survive.
        if run.text != run.text.strip():
            target._r.t.set("{http://www.w3.org/XML/1998/namespace}space", "preserve")


def _add_picture(slide, data: bytes, box: tuple, scale: float, ox: float, oy: float) -> None:
    from pptx.util import Emu

    x0, y0, x1, y1 = box
    if x1 <= x0 or y1 <= y0:
        return
    slide.shapes.add_picture(
        io.BytesIO(data),
        Emu(_emu(x0 * scale + ox)),
        Emu(_emu(y0 * scale + oy)),
        width=Emu(_emu((x1 - x0) * scale)),
        height=Emu(_emu((y1 - y0) * scale)),
    )


def _render(slide, page, box: tuple, scale: float, ox: float, oy: float) -> None:
    """Place a rasterised clip: a scan, or a picture whose codec a deck cannot hold."""
    import pymupdf as fitz
    from pptx.util import Emu

    x0, y0, x1, y1 = box
    if x1 <= x0 or y1 <= y0:
        return
    pixmap = page.get_pixmap(clip=fitz.Rect(x0, y0, x1, y1), dpi=RENDER_DPI, alpha=False)
    slide.shapes.add_picture(
        io.BytesIO(pixmap.tobytes("png")),
        Emu(_emu(x0 * scale + ox)),
        Emu(_emu(y0 * scale + oy)),
        width=Emu(_emu((x1 - x0) * scale)),
        height=Emu(_emu((y1 - y0) * scale)),
    )


def _add_rule(slide, rule, scale: float, ox: float, oy: float) -> None:
    from pptx.dml.color import RGBColor
    from pptx.enum.shapes import MSO_CONNECTOR
    from pptx.util import Emu, Pt

    line = slide.shapes.add_connector(
        MSO_CONNECTOR.STRAIGHT,
        Emu(_emu(rule.x0 * scale + ox)),
        Emu(_emu(rule.y * scale + oy)),
        Emu(_emu(rule.x1 * scale + ox)),
        Emu(_emu(rule.y * scale + oy)),
    )
    line.line.color.rgb = RGBColor(0, 0, 0)
    line.line.width = Pt(RULE_PT)


def _draw_page(slide, page, page_layout, scale: float, ox: float, oy: float,
               body_size: float) -> None:
    """Place the page's rules, pictures and one text box per visual segment."""
    boxes = _span_boxes(page)

    for rule in page_layout.rules:
        _add_rule(slide, rule, scale, ox, oy)

    for picture in page_layout.pictures:
        box = (picture.x, picture.y, picture.x + picture.width, picture.y + picture.height)
        try:
            _add_picture(slide, picture.data, box, scale, ox, oy)
        except Exception:  # noqa: BLE001 - JBIG2/JPX bytes still have pixels
            _render(slide, page, box, scale, ox, oy)

    for row in page_layout.rows:
        for segment in row.segments:
            group: list[reflow.Run] = []
            group_x0 = 0.0
            for run, box in _items(segment, boxes.get(_key(segment), [])):
                if run.image is not None:
                    if group:
                        _add_text(slide, group, group_x0, segment, scale, ox, oy, body_size)
                        group, group_x0 = [], 0.0
                    _add_picture(
                        slide,
                        run.image,
                        (
                            box[0] - 0.6,
                            box[1] - 0.6,
                            box[0] - 0.6 + (run.image_width or (box[2] - box[0]) + 1.2),
                            box[1] - 0.6 + (run.image_height or (box[3] - box[1]) + 1.2),
                        ),
                        scale,
                        ox,
                        oy,
                    )
                    continue
                if not group:
                    group_x0 = box[0]
                group.append(run)
            if group:
                _add_text(slide, group, group_x0, segment, scale, ox, oy, body_size)


def _has_text(page_layout: layout_mod.PageLayout) -> bool:
    return any(
        segment.text.strip() for row in page_layout.rows for segment in row.segments
    )


def convert(source: str, target: str) -> None:
    import pymupdf as fitz
    from pptx import Presentation
    from pptx.util import Emu

    with fitz.open(source) as document:
        if document.page_count == 0:
            raise ValueError("the PDF has no pages")
        layout_data = reflow.measure(document)

        first = document[0].rect
        slide_width = first.width or 612.0
        slide_height = first.height or 792.0
        presentation = Presentation()
        presentation.slide_width = Emu(_emu(slide_width))
        presentation.slide_height = Emu(_emu(slide_height))

        for page in document:
            rect = page.rect
            # A later page of another shape is fitted inside the deck's canvas and
            # centred; a page of the first page's shape is drawn 1:1.
            scale = 1.0
            if rect.width and rect.height:
                scale = min(slide_width / rect.width, slide_height / rect.height)
            offset_x = (slide_width - rect.width * scale) / 2
            offset_y = (slide_height - rect.height * scale) / 2

            slide = presentation.slides.add_slide(presentation.slide_layouts[6])
            page_layout = layout_mod.extract(page, layout_data)
            # A /Rotate page reports its text and images in the unrotated space while
            # `page.rect` is the turned one; rebuilding it shape by shape would need
            # every box turned as well, so it keeps the render (as it always has).
            if _has_text(page_layout) and not page.rotation:
                _draw_page(
                    slide,
                    page,
                    page_layout,
                    scale,
                    offset_x,
                    offset_y,
                    layout_data.body_size,
                )
            else:
                # A page with no text (a scan), or a turned page: only a render of it
                # keeps every pixel.
                _render(
                    slide,
                    page,
                    (rect.x0, rect.y0, rect.x1, rect.y1),
                    scale,
                    offset_x,
                    offset_y,
                )

        presentation.save(target)

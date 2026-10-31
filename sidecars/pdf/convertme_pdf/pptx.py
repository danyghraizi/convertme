"""PDF → PPTX: one slide per page, with the page rendered as a full-bleed image.

That is what preserves the original layout exactly; the alternative (reflowing text
into placeholders) loses design fidelity on every real-world PDF.

The slide size follows the document: a 16:9 deck stays 16:9, a portrait page gets a
portrait slide, and the image is fitted inside it with its own aspect ratio. A fixed
16:9 canvas letterboxed every other document into black bars.
"""

from __future__ import annotations

import io

# PowerPoint's own 16:9 canvas, in EMU.
SLIDE_WIDE = (12192000, 6858000)
RENDER_DPI = 200
# Below this, a rendered page is too soft to project; the aspect is upscaled instead.
MIN_IMAGE_WIDTH = 1600


def _slide_size(aspect: float) -> tuple[int, int]:
    """The largest PowerPoint canvas with this page's aspect ratio."""
    width, height = SLIDE_WIDE
    if aspect >= 1.0:
        return width, int(round(width / aspect))
    return int(round(height * aspect)), height


def convert(source: str, target: str) -> None:
    import pymupdf as fitz
    from pptx import Presentation
    from pptx.util import Emu

    with fitz.open(source) as document:
        if document.page_count == 0:
            raise ValueError("the PDF has no pages")
        # One aspect for the whole deck: the first page's, or 16:9 when the pages
        # disagree (a deck cannot have two canvases).
        first = document[0].rect
        aspect = (first.width / first.height) if first.height else 16 / 9
        for page in document:
            rect = page.rect
            if rect.height and abs((rect.width / rect.height) - aspect) > 0.01:
                aspect = SLIDE_WIDE[0] / SLIDE_WIDE[1]
                break

        width, height = _slide_size(aspect)
        presentation = Presentation()
        presentation.slide_width = Emu(width)
        presentation.slide_height = Emu(height)

        for page in document:
            pixmap = page.get_pixmap(dpi=RENDER_DPI, alpha=False)
            image = io.BytesIO(pixmap.tobytes("png"))
            slide = presentation.slides.add_slide(presentation.slide_layouts[6])
            # Fit inside the canvas, centred: the page keeps its own proportions even
            # when the deck's canvas is a different shape.
            page_aspect = pixmap.width / pixmap.height
            slide_aspect = width / height
            if page_aspect >= slide_aspect:
                picture_width = width
                picture_height = int(round(width / page_aspect))
            else:
                picture_height = height
                picture_width = int(round(height * page_aspect))
            left = int(round((width - picture_width) / 2))
            top = int(round((height - picture_height) / 2))
            slide.shapes.add_picture(
                image,
                Emu(left),
                Emu(top),
                width=Emu(picture_width),
                height=Emu(picture_height),
            )

        presentation.save(target)

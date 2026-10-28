"""PDF → PPTX: one slide per page, with the page rendered as a full-bleed image.

That is what preserves the original layout exactly; the alternative (reflowing
text into placeholders) loses design fidelity on every real-world PDF.
"""

from __future__ import annotations

import io


def convert(source: str, target: str) -> None:
    import pymupdf as fitz
    from pptx import Presentation
    from pptx.util import Emu

    # 16:9, matching what presentation templates expect.
    width = Emu(12192000)
    height = Emu(6858000)
    presentation = Presentation()
    presentation.slide_width = width
    presentation.slide_height = height

    with fitz.open(source) as document:
        if document.page_count == 0:
            raise ValueError("the PDF has no pages")
        for page in document:
            # 200 dpi keeps text readable when projected or printed.
            pixmap = page.get_pixmap(dpi=200, alpha=False)
            image = io.BytesIO(pixmap.tobytes("png"))
            slide = presentation.slides.add_slide(presentation.slide_layouts[6])
            slide.shapes.add_picture(image, 0, 0, width=width, height=height)

    presentation.save(target)

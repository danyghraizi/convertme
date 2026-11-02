"""PDF → office/EPUB/Markdown conversions, isolated in a sidecar.

The Rust side owns planning, queuing and publishing; this package only turns one
PDF into one file. That keeps the heavy OCR/layout dependencies out of the app
binary and out of a conversion that does not need them.
"""

__all__ = ["docx", "pptx", "xlsx", "epub", "md"]

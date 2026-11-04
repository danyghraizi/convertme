# PyInstaller spec for `convertme-pdf`.
#
# Two big payloads are pruned on purpose:
#   * `pymupdf/layout/resources/onnx` (~49 MB) — PyMuPDF 2.x ships neural layout
#     models for `find_tables`; the sidecar uses the classic table finder, so the
#     weights are dead weight in a 320 MB product.
#   * `onnxruntime` (~30 MB) — only reachable through those models.
#
# Build with `python build.py`, which supplies the embedded source stamp.

import os
import sys

from PyInstaller.utils.hooks import collect_data_files, collect_submodules

block_cipher = None

# Data files, minus the neural layout models.
datas = [
    entry
    for entry in collect_data_files("pymupdf")
    if "layout/resources" not in entry[0].replace(os.sep, "/")
]
datas.append((os.environ["CONVERTME_PDF_STAMP_FILE"], "."))


hiddenimports = (
    collect_submodules("pymupdf")
    + collect_submodules("docx")
    + collect_submodules("pptx")
    + collect_submodules("openpyxl")
)

excludes = [
    "matplotlib",
    "tkinter",
    "scipy",
    "pandas",
    "PyQt5",
    "PySide6",
    "IPython",
    "pytest",
    "pdf2docx",
    "cv2",
    "numpy",
    "onnxruntime",
]

a = Analysis(
    ["__main__runner.py"],
    pathex=[os.path.abspath(".")],
    binaries=[],
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=excludes,
    noarchive=False,
)

pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name="convertme-pdf",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=True,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)

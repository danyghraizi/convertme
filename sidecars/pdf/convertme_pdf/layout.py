"""A page as visual rows, not PyMuPDF text blocks.

PyMuPDF groups text into blocks that follow the content stream, not the page. A
résumé's two-column header, its four-column coursework and the date on the right of
an employer all arrive as separate blocks, and a block can even contain the end of
one section followed by the next heading. Walking blocks therefore emitted
"Sep. 2017  May 2021 Bachelor ... City, State" as one paragraph and ordered the
coursework down each column instead of across each row.

This module flattens the page to lines, groups lines whose baselines overlap into a
visual row, and classifies each row: title, section heading, dated entry, detail
line, project, skill row, bullet, multi-column bullet grid, or body text. The DOCX
writer turns those rows into Word paragraphs, tab stops and a borderless table.

The reflow module remains the EPUB writer's structure analyser; this is the
layout-aware path DOCX needs to look like the page.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field

from . import reflow

DATE = re.compile(r"\b(?:19|20)\d{2}\b")
HEADING_CHARS = 90


@dataclass
class Segment:
    """One extracted line: the runs, their box, and the line's role signals."""

    runs: list[reflow.Run]
    x0: float
    y0: float
    x1: float
    y1: float
    text: str
    size: float
    bold: bool
    italic: bool
    bullet: bool

    @property
    def has_bold(self) -> bool:
        return any(run.bold for run in self.runs if run.text.strip())

    @property
    def has_italic(self) -> bool:
        return any(run.italic for run in self.runs if run.text.strip())

    @property
    def has_image(self) -> bool:
        return any(run.image for run in self.runs)

    @property
    def has_link(self) -> bool:
        return any(run.link for run in self.runs)


@dataclass
class Row:
    segments: list[Segment] = field(default_factory=list)

    def add(self, segment: Segment) -> None:
        self.segments.append(segment)
        self.segments.sort(key=lambda item: item.x0)

    @property
    def x0(self) -> float:
        return min(segment.x0 for segment in self.segments)

    @property
    def x1(self) -> float:
        return max(segment.x1 for segment in self.segments)

    @property
    def y0(self) -> float:
        return min(segment.y0 for segment in self.segments)

    @property
    def y1(self) -> float:
        return max(segment.y1 for segment in self.segments)

    @property
    def text(self) -> str:
        return " ".join(segment.text.strip() for segment in self.segments if segment.text.strip())

    @property
    def has_image(self) -> bool:
        return any(segment.has_image for segment in self.segments)

    @property
    def has_link(self) -> bool:
        return any(segment.has_link for segment in self.segments)


@dataclass
class Picture:
    data: bytes
    x: float
    y: float
    width: float
    height: float


@dataclass
class Rule:
    y: float
    x0: float
    x1: float


@dataclass
class PageLayout:
    rows: list[Row]
    pictures: list[Picture]
    rules: list[Rule]


def _segment(page, line, layout: reflow.Layout, links) -> Segment | None:
    runs = reflow._join_runs(reflow._runs_for_line(page, line, layout, links))
    text = "".join(run.text for run in runs)
    if not text.strip() and not any(run.image for run in runs):
        return None
    x0, y0, x1, y1 = (float(value) for value in line["bbox"])
    size = reflow._dominant_size(line.get("spans", []))
    bullet, _ = reflow._bullet_text(text)
    body_runs = [run for run in runs if run.text.strip() and not run.image]
    return Segment(
        runs=runs,
        x0=x0,
        y0=y0,
        x1=x1,
        y1=y1,
        text=text,
        size=size,
        bold=bool(body_runs) and all(run.bold for run in body_runs),
        italic=bool(body_runs) and all(run.italic for run in body_runs),
        bullet=bullet,
    )


def _same_row(row: Row, segment: Segment) -> bool:
    """Two lines share a row when their boxes overlap vertically or line up."""
    overlap = min(row.y1, segment.y1) - max(row.y0, segment.y0)
    shortest = min(row.y1 - row.y0, segment.y1 - segment.y0)
    if shortest > 0 and overlap / shortest >= 0.6:
        return True
    row_center = (row.y0 + row.y1) / 2
    segment_center = (segment.y0 + segment.y1) / 2
    return abs(row_center - segment_center) <= 2.5


def _rows(segments: list[Segment]) -> list[Row]:
    rows: list[Row] = []
    for segment in sorted(segments, key=lambda item: (item.y0, item.x0)):
        for row in reversed(rows[-3:]):
            if _same_row(row, segment):
                row.add(segment)
                break
        else:
            row = Row()
            row.add(segment)
            rows.append(row)
    rows.sort(key=lambda row: (row.y0, row.x0))
    return rows


def extract(page, layout: reflow.Layout) -> PageLayout:
    """Flatten one page into visual rows, pictures and horizontal rules."""
    links = reflow._links_for(page)
    segments: list[Segment] = []
    for block in page.get_text("dict", sort=True).get("blocks", []):
        if block.get("type") == 1:
            continue
        for line in block.get("lines", []):
            segment = _segment(page, line, layout, links)
            if segment is not None:
                segments.append(segment)

    pictures: list[Picture] = []
    # The text-dict image block's `number` is not an xref in PyMuPDF 2.0
    # (`Pixmap(page.parent, number)` raises "bad xref"). The image-info walk gives
    # the real xref and box, and the document can extract the original bytes.
    try:
        for info in page.get_image_info(xrefs=True):
            xref = info.get("xref")
            if not xref:
                continue
            try:
                image = page.parent.extract_image(int(xref))
            except Exception:  # noqa: BLE001 - a broken image must not fail the export
                continue
            x0, y0, x1, y1 = (float(value) for value in info["bbox"])
            pictures.append(
                Picture(
                    data=image["image"],
                    x=x0,
                    y=y0,
                    width=x1 - x0,
                    height=y1 - y0,
                )
            )
    except Exception:  # noqa: BLE001 - image discovery is best effort
        pass

    rules: list[Rule] = []
    try:
        for drawing in page.get_drawings():
            rect = drawing["rect"]
            if rect.width > page.rect.width * 0.5 and rect.height <= 2:
                rules.append(Rule(y=float(rect.y0), x0=float(rect.x0), x1=float(rect.x1)))
    except Exception:  # noqa: BLE001 - rules are decoration
        pass

    return PageLayout(rows=_rows(segments), pictures=pictures, rules=rules)


@dataclass
class Header:
    title: list[reflow.Run]
    address: list[list[reflow.Run]]
    contact: list[list[reflow.Run]]
    contact_x: list[float]
    photo: Picture | None
    x: float = 0.0


@dataclass
class Block:
    kind: str  # heading, entry, detail, project, skill, bullet, body, grid, picture
    runs: list[reflow.Run] = field(default_factory=list)
    right: list[reflow.Run] = field(default_factory=list)
    left: list[reflow.Run] = field(default_factory=list)
    level: int = 1
    border: bool = False
    cells: list[list[list[reflow.Run]]] = field(default_factory=list)
    picture: Picture | None = None
    y: float = 0.0
    left_x: float = 0.0


def _runs_of(row: Row) -> list[reflow.Run]:
    runs: list[reflow.Run] = []
    for segment in row.segments:
        if runs and segment.text.strip() and not "".join(r.text for r in runs).endswith(" "):
            runs.append(reflow.Run(text=" "))
        runs.extend(segment.runs)
    return runs


def _right_aligned(segment: Segment, content_right: float) -> bool:
    return segment.x1 >= content_right - 8 and not segment.bullet


def _heading_level(row: Row, layout: reflow.Layout) -> int | None:
    if len(row.text) > HEADING_CHARS:
        return None
    for segment in row.segments:
        rounded = round(segment.size, 1)
        if rounded in layout.heading_levels:
            return layout.heading_levels[rounded]
    return None


def _border(row: Row, rules: list[Rule], page_width: float) -> bool:
    for rule in rules:
        if (
            rule.x1 - rule.x0 > page_width * 0.5
            and row.y1 - 1 <= rule.y <= row.y1 + 18
        ):
            return True
    return False


def _strip_prefix(runs: list[reflow.Run]) -> list[reflow.Run]:
    """Remove the bullet glyph from the first text run, keeping every format."""
    out = [reflow.Run(**{**run.__dict__}) for run in runs]
    for run in out:
        if not run.text:
            continue
        text = run.text.lstrip()
        if text and text[0] in reflow.BULLET_GLYPHS:
            text = text[1:].lstrip()
        for prefix in reflow.BULLET_PREFIX:
            if text.startswith(prefix + " "):
                text = text[len(prefix) + 1 :].lstrip()
                break
        run.text = text
        break
    return [run for run in out if run.text or run.image]


def frame(page, layout: reflow.Layout) -> tuple[Header, list[Block], list[Row]]:
    """Split a page into its header rows and the content blocks under them."""
    page_layout = extract(page, layout)
    rows = page_layout.rows
    content_right = page.rect.x1 - 28.8

    first_heading = None
    for index, row in enumerate(rows):
        level = _heading_level(row, layout)
        if level is not None and level >= 1 and len(row.text.strip()) <= HEADING_CHARS:
            first_heading = index
            break
    header_rows = rows[:first_heading] if first_heading is not None else []
    body_rows = rows[first_heading:] if first_heading is not None else rows

    title: list[reflow.Run] = []
    address: list[list[reflow.Run]] = []
    contact: list[list[reflow.Run]] = []
    contact_x: list[float] = []
    for row in header_rows:
        if row.has_image or row.has_link:
            for segment in row.segments:
                contact.append(segment.runs)
                contact_x.append(segment.x0)
            continue
        if layout.title_size is not None and any(
            abs(segment.size - layout.title_size) < 0.2 for segment in row.segments
        ):
            title = _runs_of(row)
        else:
            address.append(_runs_of(row))

    photo = None
    for picture in page_layout.pictures:
        if picture.y < (header_rows[-1].y1 if header_rows else page.rect.height):
            if picture.width < page.rect.width * 0.4:
                photo = picture
                break

    blocks: list[Block] = []
    current: Block | None = None
    last_bullet: Block | None = None

    def flush() -> None:
        nonlocal current, last_bullet
        current = None
        last_bullet = None

    index = 0
    while index < len(body_rows):
        row = body_rows[index]
        level = _heading_level(row, layout)
        if level is not None and len(row.text.strip()) <= HEADING_CHARS:
            flush()
            blocks.append(
                Block(
                    kind="heading",
                    runs=_runs_of(row),
                    level=max(1, level),
                    border=_border(row, page_layout.rules, page.rect.width),
                    y=row.y0,
                )
            )
            index += 1
            continue

        grid = [segment for segment in row.segments if segment.bullet]
        if len(grid) >= 2:
            flush()
            grid_rows: list[Row] = []
            while index < len(body_rows):
                candidate = body_rows[index]
                bullets = [segment for segment in candidate.segments if segment.bullet]
                if len(bullets) < 2:
                    break
                grid_rows.append(candidate)
                index += 1
            columns = max(len(candidate.segments) for candidate in grid_rows)
            cells = [
                [
                    _strip_prefix(candidate.segments[column].runs)
                    if column < len(candidate.segments)
                    else []
                    for column in range(columns)
                ]
                for candidate in grid_rows
            ]
            blocks.append(Block(kind="grid", cells=cells, y=grid_rows[0].y0))
            continue

        if any(segment.bullet for segment in row.segments):
            flush()
            segment = next(segment for segment in row.segments if segment.bullet)
            bullet = Block(
                kind="bullet",
                runs=_strip_prefix(segment.runs),
                y=row.y0,
                left_x=segment.x0,
            )
            blocks.append(bullet)
            current = None
            last_bullet = bullet
            index += 1
            continue

        last = row.segments[-1]
        right_aligned = len(row.segments) >= 2 and _right_aligned(last, content_right)
        if right_aligned:
            flush()
            left_runs: list[reflow.Run] = []
            for segment in row.segments[:-1]:
                if left_runs:
                    left_runs.append(reflow.Run(text=" "))
                left_runs.extend(segment.runs)
            right_runs = list(last.runs)
            left = row.segments[0]
            # The left side decides the row's role. A project line carries both a
            # bold name and italic technology; an employer/school line is bold only;
            # the italic detail under either is a single italic phrase.
            if left.has_italic and left.has_bold:
                kind = "project"
            elif left.has_bold:
                kind = "entry"
            elif left.has_italic:
                kind = "detail"
            elif last.has_italic:
                kind = "detail"
            else:
                kind = "project"
            blocks.append(
                Block(
                    kind=kind,
                    left=left_runs,
                    right=right_runs,
                    y=row.y0,
                )
            )
            index += 1
            continue

        segment = row.segments[0]
        skill = (
            segment.has_bold
            and row.text.strip().find(":") >= 0
            and row.segments[0].text.strip()[:1].isupper()
        )
        if skill:
            flush()
            blocks.append(Block(kind="skill", runs=_runs_of(row), y=row.y0))
            index += 1
            continue

        # A short standalone bold line is a sub-heading (a section label that is
        # set at body size, as in a plain report). A bold label with a colon is a
        # skill row and is handled below, not here.
        first_segment = row.segments[0]
        if (
            first_segment.has_bold
            and not first_segment.has_italic
            and len(row.text.strip()) <= HEADING_CHARS
            and ":" not in row.text
        ):
            flush()
            blocks.append(
                Block(kind="heading", runs=_runs_of(row), level=2, y=row.y0)
            )
            index += 1
            continue

        # A wrapped line of the open bullet is indented under its text and has no
        # marker of its own. Keep it in the same list item instead of turning the
        # tail of every bullet into a body paragraph.
        if (
            last_bullet is not None
            and not row.segments[0].has_bold
            and row.segments[0].x0 > last_bullet.left_x + 3
            and row.y0 - (body_rows[index - 1].y0 if index else 0) < layout.body_size * 1.8
        ):
            last_bullet.runs.append(reflow.Run(text=" "))
            last_bullet.runs.extend(_runs_of(row))
            index += 1
            continue

        if (
            current is not None
            and not row.segments[0].has_bold
            and abs(row.segments[0].x0 - current.left_x) < 8
            and row.y0 - (body_rows[index - 1].y0 if index else 0)
            <= layout.body_size * 1.8
        ):
            current.runs.append(reflow.Run(text=" "))
            current.runs.extend(_runs_of(row))
            index += 1
            continue

        flush()
        current = Block(
            kind="body",
            runs=_runs_of(row),
            y=row.y0,
            left_x=row.segments[0].x0,
        )
        blocks.append(current)
        index += 1

    # Body pictures that are not the header photo join the stream at their page
    # position. The header photo is written into the header table instead.
    if photo is not None:
        page_layout.pictures = [
            picture
            for picture in page_layout.pictures
            if (picture.x, picture.y, picture.width, picture.height)
            != (photo.x, photo.y, photo.width, photo.height)
        ]
    for picture in page_layout.pictures:
        blocks.append(Block(kind="picture", picture=picture, y=picture.y))
    blocks.sort(key=lambda block: block.y)
    header_x = photo.x if photo is not None else (contact_x[0] if contact_x else 0.0)
    if photo is None and title:
        for row in header_rows:
            if layout.title_size is not None and any(abs(segment.size - layout.title_size) < 0.2 for segment in row.segments):
                header_x = row.x0
                break
    return Header(title, address, contact, contact_x, photo, header_x), blocks, body_rows

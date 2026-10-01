#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "Pillow>=10.0",
# ]
# ///
"""Convert PNG brush-tip images into HoloPainter raw R8 + texture RON files.

The generated .r8 file is headerless, row-major, one byte per pixel. A value of
0 is transparent/no paint and 255 is fully opaque/full paint.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import tempfile
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence

try:
    from PIL import Image, ImageChops, ImageOps, UnidentifiedImageError
except ImportError as exc:  # pragma: no cover - depends on the user's environment
    raise SystemExit(
        "Pillow is required. Install it with: python -m pip install Pillow"
    ) from exc


EXIT_OK = 0
EXIT_CONVERSION_ERROR = 1
EXIT_ARGUMENT_ERROR = 2
EXIT_CONFLICT = 3
MAX_U32 = 0xFFFF_FFFF
RESOURCE_ID_RE = re.compile(r'\bid\s*:\s*"((?:\\.|[^"\\])*)"')
VALID_RESOURCE_ID_RE = re.compile(r"^[a-z0-9_.]+$")


class ConversionError(RuntimeError):
    """An input could not be converted or an output could not be written."""


class ConflictError(ConversionError):
    """An output path or resource ID conflicts with an existing resource."""


@dataclass(frozen=True)
class Config:
    output_dir: Path
    mode: str
    canvas: str
    anchor: str
    padding_value: int
    mipmaps: bool
    id_prefix: str
    overwrite: bool
    flip_y: bool
    max_dimension: int
    preview_dir: Path | None
    dry_run: bool


@dataclass(frozen=True)
class ConversionPlan:
    source: Path
    source_mode: str
    source_size: tuple[int, int]
    selected_mode: str
    output_size: tuple[int, int]
    offset: tuple[int, int]
    normalized_name: str
    display_name: str
    resource_id: str
    r8_path: Path
    ron_path: Path
    preview_path: Path | None
    warning: str | None


@dataclass(frozen=True)
class ConversionResult:
    plan: ConversionPlan
    byte_count: int


def parse_bool(value: str) -> bool:
    normalized = value.strip().lower()
    if normalized in {"true", "1", "yes", "on"}:
        return True
    if normalized in {"false", "0", "no", "off"}:
        return False
    raise argparse.ArgumentTypeError("expected true or false")


def existing_path(value: str) -> Path:
    path = Path(value).expanduser()
    if not path.exists():
        raise argparse.ArgumentTypeError(f"input does not exist: {path}")
    return path


def bounded_byte(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("expected an integer from 0 to 255") from exc
    if not 0 <= parsed <= 255:
        raise argparse.ArgumentTypeError("expected an integer from 0 to 255")
    return parsed


def positive_int(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("expected a positive integer") from exc
    if parsed <= 0:
        raise argparse.ArgumentTypeError("expected a positive integer")
    return parsed


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Convert PNG brush-tip images into headerless R8 data and "
            "HoloPainter .texture.ron definitions."
        )
    )
    parser.add_argument(
        "inputs",
        metavar="INPUT",
        nargs="+",
        type=existing_path,
        help="PNG file or directory containing PNG files",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("resources/textures/brush_tips"),
        help="output directory (default: resources/textures/brush_tips)",
    )
    parser.add_argument(
        "--mode",
        choices=("auto", "alpha", "inverse-luma", "luma", "alpha-inverse-luma"),
        default="auto",
        help="mask conversion mode (default: auto)",
    )
    parser.add_argument(
        "--canvas",
        choices=("preserve", "square"),
        default="square",
        help="preserve dimensions or pad to a square canvas (default: square)",
    )
    parser.add_argument(
        "--anchor",
        choices=("center",),
        default="center",
        help="placement on a padded canvas (default: center)",
    )
    parser.add_argument(
        "--padding-value",
        type=bounded_byte,
        default=0,
        metavar="0..255",
        help="R8 value used for square-canvas padding (default: 0)",
    )
    parser.add_argument(
        "--mipmaps",
        type=parse_bool,
        default=True,
        metavar="true|false",
        help="value written to the RON mipmaps field (default: true)",
    )
    parser.add_argument(
        "--id-prefix",
        default="texture.brush_tip",
        help="resource ID prefix (default: texture.brush_tip)",
    )
    parser.add_argument(
        "--display-name",
        help="display name override; valid only when converting one PNG",
    )
    parser.add_argument(
        "--name",
        help="output basename override; valid only when converting one PNG",
    )
    parser.add_argument(
        "--overwrite",
        action="store_true",
        help="replace existing output files belonging to the same resource",
    )
    parser.add_argument(
        "--recursive",
        action="store_true",
        help="search input directories recursively",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="show conversion plans without writing files",
    )
    parser.add_argument(
        "--flip-y",
        action="store_true",
        help="vertically flip the mask before writing R8 data",
    )
    parser.add_argument(
        "--max-dimension",
        type=positive_int,
        default=4096,
        help="maximum output width or height (default: 4096)",
    )
    parser.add_argument(
        "--preview-dir",
        type=Path,
        help="optionally write grayscale PNG previews of final R8 masks",
    )
    return parser


def discover_pngs(inputs: Sequence[Path], recursive: bool) -> list[Path]:
    discovered: dict[Path, None] = {}
    for input_path in inputs:
        path = input_path.expanduser().resolve()
        if path.is_file():
            if path.suffix.lower() != ".png":
                raise ConversionError(f"input is not a PNG file: {input_path}")
            discovered[path] = None
            continue

        if not path.is_dir():
            raise ConversionError(f"input is neither a file nor directory: {input_path}")

        iterator: Iterable[Path]
        iterator = path.rglob("*") if recursive else path.iterdir()
        for candidate in iterator:
            if candidate.is_file() and candidate.suffix.lower() == ".png":
                discovered[candidate.resolve()] = None

    result = sorted(discovered, key=lambda item: str(item).casefold())
    if not result:
        raise ConversionError("no PNG files were found")
    return result


def normalize_name(value: str) -> str:
    normalized = unicodedata.normalize("NFKC", value).lower()
    normalized = re.sub(r"[\s-]+", "_", normalized)
    normalized = re.sub(r"[^a-z0-9_]", "_", normalized)
    normalized = re.sub(r"_+", "_", normalized).strip("_")
    if not normalized:
        raise ConversionError(
            f"cannot derive an ASCII resource name from {value!r}; use --name"
        )
    return normalized


def validate_resource_id_prefix(prefix: str) -> str:
    normalized = prefix.strip().lower().strip(".")
    if not normalized or not VALID_RESOURCE_ID_RE.fullmatch(normalized):
        raise ConversionError(
            "--id-prefix may contain only lowercase a-z, digits, underscore, and dot"
        )
    if ".." in normalized:
        raise ConversionError("--id-prefix must not contain consecutive dots")
    return normalized


def has_meaningful_alpha(image: Image.Image) -> bool:
    has_alpha_channel = "A" in image.getbands() or "transparency" in image.info
    if not has_alpha_channel:
        return False
    alpha = image.convert("RGBA").getchannel("A")
    low, high = alpha.getextrema()
    return low != 255 or high != 255


def select_mode(image: Image.Image, requested_mode: str) -> str:
    if requested_mode != "auto":
        return requested_mode
    return "alpha" if has_meaningful_alpha(image) else "inverse-luma"


def rec709_luma(image: Image.Image) -> Image.Image:
    rgb = image.convert("RGB")
    # Pillow applies this 4-tuple matrix when converting RGB to L.
    return rgb.convert("L", matrix=(0.2126, 0.7152, 0.0722, 0.0))


def make_mask(image: Image.Image, mode: str) -> Image.Image:
    if mode == "alpha":
        if "A" not in image.getbands() and "transparency" not in image.info:
            raise ConversionError("alpha mode requires an alpha channel or PNG transparency")
        return image.convert("RGBA").getchannel("A")

    luma = rec709_luma(image)
    if mode == "luma":
        return luma
    if mode == "inverse-luma":
        return ImageOps.invert(luma)
    if mode == "alpha-inverse-luma":
        if "A" not in image.getbands() and "transparency" not in image.info:
            raise ConversionError(
                "alpha-inverse-luma mode requires an alpha channel or PNG transparency"
            )
        alpha = image.convert("RGBA").getchannel("A")
        return ImageChops.multiply(alpha, ImageOps.invert(luma))
    raise AssertionError(f"unhandled mode: {mode}")


def validate_dimensions(width: int, height: int, max_dimension: int, label: str) -> None:
    if width <= 0 or height <= 0:
        raise ConversionError(f"{label} dimensions must be at least 1x1")
    if width > max_dimension or height > max_dimension:
        raise ConversionError(
            f"{label} dimensions {width}x{height} exceed --max-dimension {max_dimension}"
        )
    if width * height > MAX_U32:
        raise ConversionError(f"{label} pixel count exceeds u32 range: {width}x{height}")


def apply_canvas(
    mask: Image.Image, canvas: str, anchor: str, padding_value: int
) -> tuple[Image.Image, tuple[int, int], str | None]:
    width, height = mask.size
    if canvas == "preserve" or width == height:
        warning = None
        if canvas == "preserve" and width != height:
            warning = (
                f"non-square texture {width}x{height} will be sampled over a square "
                "brush footprint"
            )
        return mask, (0, 0), warning

    if anchor != "center":
        raise AssertionError(f"unhandled anchor: {anchor}")
    side = max(width, height)
    offset = ((side - width) // 2, (side - height) // 2)
    padded = Image.new("L", (side, side), color=padding_value)
    padded.paste(mask, offset)
    return padded, offset, None


def ron_escape(value: str) -> str:
    return (
        value.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace("\t", "\\t")
    )


def ron_unescape(value: str) -> str:
    # Resource IDs are restricted to a safe ASCII subset when generated. This
    # decoder only needs enough coverage to compare IDs in existing RON files.
    return re.sub(r"\\([\\\"])", r"\1", value)


def render_ron(plan: ConversionPlan, mipmaps: bool) -> str:
    width, height = plan.output_size
    return (
        "(\n"
        "    schema_version: 1,\n"
        f'    id: "{ron_escape(plan.resource_id)}",\n'
        f'    display_name: "{ron_escape(plan.display_name)}",\n'
        "    source: File(\n"
        f'        path: "{ron_escape(plan.r8_path.name)}",\n'
        f"        decode: RawR8(width: {width}, height: {height}),\n"
        "    ),\n"
        "    format: R8Unorm,\n"
        f"    mipmaps: {'true' if mipmaps else 'false'},\n"
        '    tags: ["brush_tip_mask"],\n'
        ")\n"
    )


def validate_ron_text(plan: ConversionPlan, ron_text: str, mipmaps: bool) -> None:
    width, height = plan.output_size
    required_fragments = (
        "schema_version: 1",
        f'id: "{ron_escape(plan.resource_id)}"',
        f'path: "{ron_escape(plan.r8_path.name)}"',
        f"RawR8(width: {width}, height: {height})",
        "format: R8Unorm",
        f"mipmaps: {'true' if mipmaps else 'false'}",
        'tags: ["brush_tip_mask"]',
    )
    missing = [fragment for fragment in required_fragments if fragment not in ron_text]
    if missing:
        raise ConversionError(
            "internal RON validation failed; missing: " + ", ".join(missing)
        )


def find_texture_root(output_dir: Path) -> Path:
    resolved = output_dir.expanduser().resolve()
    current = resolved
    while True:
        if current.name == "textures" and current.parent.name == "resources":
            return current
        if current.parent == current:
            return resolved
        current = current.parent


def read_existing_ids(root: Path) -> dict[str, Path]:
    ids: dict[str, Path] = {}
    if not root.exists():
        return ids
    for ron_path in root.rglob("*.texture.ron"):
        try:
            text = ron_path.read_text(encoding="utf-8")
        except OSError as exc:
            raise ConversionError(f"cannot read existing RON {ron_path}: {exc}") from exc
        match = RESOURCE_ID_RE.search(text)
        if match:
            resource_id = ron_unescape(match.group(1))
            ids.setdefault(resource_id, ron_path.resolve())
    return ids


def plan_conversion(
    source: Path,
    config: Config,
    existing_ids: dict[str, Path],
    explicit_name: str | None,
    explicit_display_name: str | None,
) -> tuple[ConversionPlan, Image.Image]:
    try:
        with Image.open(source) as opened:
            source_mode = opened.mode
            source_size = opened.size
            validate_dimensions(*source_size, config.max_dimension, "input")
            opened.load()
            selected_mode = select_mode(opened, config.mode)
            mask = make_mask(opened, selected_mode)
    except UnidentifiedImageError as exc:
        raise ConversionError(f"cannot decode PNG: {source}") from exc
    except Image.DecompressionBombError as exc:
        raise ConversionError(f"image is too large or unsafe to decode: {source}") from exc
    except OSError as exc:
        raise ConversionError(f"cannot read PNG {source}: {exc}") from exc

    mask, offset, warning = apply_canvas(
        mask, config.canvas, config.anchor, config.padding_value
    )
    if config.flip_y:
        mask = ImageOps.flip(mask)
    validate_dimensions(*mask.size, config.max_dimension, "output")

    normalized_name = normalize_name(explicit_name or source.stem)
    display_name = explicit_display_name if explicit_display_name is not None else source.stem
    if not display_name:
        raise ConversionError("display name must not be empty")

    resource_id = f"{config.id_prefix}.{normalized_name}"
    if not VALID_RESOURCE_ID_RE.fullmatch(resource_id) or ".." in resource_id:
        raise ConversionError(f"invalid generated resource ID: {resource_id}")

    r8_path = config.output_dir / f"{normalized_name}.r8"
    ron_path = config.output_dir / f"{normalized_name}.texture.ron"
    preview_path = (
        config.preview_dir / f"{normalized_name}.png" if config.preview_dir else None
    )

    existing_id_path = existing_ids.get(resource_id)
    same_target = existing_id_path == ron_path.expanduser().resolve()
    if existing_id_path is not None and not (config.overwrite and same_target):
        raise ConflictError(
            f"resource ID {resource_id!r} already exists in {existing_id_path}"
        )

    occupied = [path for path in (r8_path, ron_path) if path.exists()]
    if occupied and not config.overwrite:
        joined = ", ".join(str(path) for path in occupied)
        raise ConflictError(f"output already exists: {joined}; use --overwrite to replace")

    plan = ConversionPlan(
        source=source,
        source_mode=source_mode,
        source_size=source_size,
        selected_mode=selected_mode,
        output_size=mask.size,
        offset=offset,
        normalized_name=normalized_name,
        display_name=display_name,
        resource_id=resource_id,
        r8_path=r8_path,
        ron_path=ron_path,
        preview_path=preview_path,
        warning=warning,
    )
    return plan, mask


def fsync_file(file_obj: object) -> None:
    file_obj.flush()  # type: ignore[attr-defined]
    os.fsync(file_obj.fileno())  # type: ignore[attr-defined]


def write_pair_atomic(
    plan: ConversionPlan, mask: Image.Image, ron_text: str, overwrite: bool
) -> int:
    plan.r8_path.parent.mkdir(parents=True, exist_ok=True)
    r8_bytes = mask.tobytes()
    expected_size = plan.output_size[0] * plan.output_size[1]
    if len(r8_bytes) != expected_size:
        raise ConversionError(
            f"internal R8 size mismatch: got {len(r8_bytes)}, expected {expected_size}"
        )

    temp_paths: list[Path] = []
    backup_paths: dict[Path, Path] = {}
    installed_paths: list[Path] = []
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb", prefix=f".{plan.normalized_name}.", suffix=".r8.tmp",
            dir=plan.r8_path.parent, delete=False
        ) as r8_file:
            r8_file.write(r8_bytes)
            fsync_file(r8_file)
            temp_r8 = Path(r8_file.name)
            temp_paths.append(temp_r8)

        if temp_r8.stat().st_size != expected_size:
            raise ConversionError(
                f"temporary R8 size mismatch: got {temp_r8.stat().st_size}, "
                f"expected {expected_size}"
            )

        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", newline="\n",
            prefix=f".{plan.normalized_name}.", suffix=".texture.ron.tmp",
            dir=plan.ron_path.parent, delete=False
        ) as ron_file:
            ron_file.write(ron_text)
            fsync_file(ron_file)
            temp_ron = Path(ron_file.name)
            temp_paths.append(temp_ron)

        for target in (plan.r8_path, plan.ron_path):
            if target.exists():
                if not overwrite:
                    raise ConflictError(f"output already exists: {target}")
                backup_handle = tempfile.NamedTemporaryFile(
                    prefix=f".{target.name}.", suffix=".bak", dir=target.parent, delete=False
                )
                backup_path = Path(backup_handle.name)
                backup_handle.close()
                backup_path.unlink()
                os.replace(target, backup_path)
                backup_paths[target] = backup_path

        os.replace(temp_r8, plan.r8_path)
        installed_paths.append(plan.r8_path)
        temp_paths.remove(temp_r8)
        os.replace(temp_ron, plan.ron_path)
        installed_paths.append(plan.ron_path)
        temp_paths.remove(temp_ron)

        if plan.r8_path.stat().st_size != expected_size:
            raise ConversionError(
                f"written R8 size mismatch: got {plan.r8_path.stat().st_size}, "
                f"expected {expected_size}"
            )

        for backup in backup_paths.values():
            backup.unlink(missing_ok=True)
        return expected_size
    except Exception:
        for installed in reversed(installed_paths):
            installed.unlink(missing_ok=True)
        for target, backup in backup_paths.items():
            if backup.exists():
                os.replace(backup, target)
        raise
    finally:
        for temp_path in temp_paths:
            temp_path.unlink(missing_ok=True)
        for backup in backup_paths.values():
            backup.unlink(missing_ok=True)


def write_preview(path: Path, mask: Image.Image, overwrite: bool) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() and not overwrite:
        raise ConflictError(f"preview already exists: {path}; use --overwrite to replace")
    with tempfile.NamedTemporaryFile(
        prefix=f".{path.stem}.", suffix=".png.tmp", dir=path.parent, delete=False
    ) as file_obj:
        temp_path = Path(file_obj.name)
    try:
        mask.save(temp_path, format="PNG")
        os.replace(temp_path, path)
    finally:
        temp_path.unlink(missing_ok=True)


def print_result(result: ConversionResult, auto_requested: bool, dry_run: bool) -> None:
    plan = result.plan
    label = "PLAN" if dry_run else "OK"
    print(f"{label} {plan.source}")
    print(f"  source:       {plan.source_size[0]}x{plan.source_size[1]} {plan.source_mode}")
    if auto_requested:
        print(f"  mask mode:    auto -> {plan.selected_mode}")
    else:
        print(f"  mask mode:    {plan.selected_mode}")
    print(f"  canvas:       {plan.output_size[0]}x{plan.output_size[1]}")
    if plan.offset != (0, 0):
        print(f"  offset:       x={plan.offset[0]}, y={plan.offset[1]}")
    print(f"  r8 bytes:     {result.byte_count}")
    print(f"  id:           {plan.resource_id}")
    print(f"  r8:           {plan.r8_path}")
    print(f"  ron:          {plan.ron_path}")
    if plan.preview_path is not None:
        print(f"  preview:      {plan.preview_path}")
    if plan.warning:
        print(f"  warning:      {plan.warning}")


def run(args: argparse.Namespace) -> int:
    pngs = discover_pngs(args.inputs, args.recursive)
    if args.name is not None and len(pngs) != 1:
        raise ConversionError("--name is valid only when converting exactly one PNG")
    if args.display_name is not None and len(pngs) != 1:
        raise ConversionError(
            "--display-name is valid only when converting exactly one PNG"
        )

    output_dir = args.output_dir.expanduser()
    preview_dir = args.preview_dir.expanduser() if args.preview_dir else None
    id_prefix = validate_resource_id_prefix(args.id_prefix)
    config = Config(
        output_dir=output_dir,
        mode=args.mode,
        canvas=args.canvas,
        anchor=args.anchor,
        padding_value=args.padding_value,
        mipmaps=args.mipmaps,
        id_prefix=id_prefix,
        overwrite=args.overwrite,
        flip_y=args.flip_y,
        max_dimension=args.max_dimension,
        preview_dir=preview_dir,
        dry_run=args.dry_run,
    )

    texture_root = find_texture_root(output_dir)
    existing_ids = read_existing_ids(texture_root)
    converted = 0
    failed = 0
    conflict_seen = False

    # Catch collisions among files planned in the same invocation, including dry runs.
    planned_names: set[str] = set()
    planned_ids: set[str] = set()

    for source in pngs:
        try:
            plan, mask = plan_conversion(
                source=source,
                config=config,
                existing_ids=existing_ids,
                explicit_name=args.name,
                explicit_display_name=args.display_name,
            )
            if plan.normalized_name in planned_names:
                raise ConflictError(
                    f"multiple inputs produce the same output name: {plan.normalized_name}"
                )
            if plan.resource_id in planned_ids:
                raise ConflictError(
                    f"multiple inputs produce the same resource ID: {plan.resource_id}"
                )
            planned_names.add(plan.normalized_name)
            planned_ids.add(plan.resource_id)

            ron_text = render_ron(plan, config.mipmaps)
            validate_ron_text(plan, ron_text, config.mipmaps)
            expected_size = plan.output_size[0] * plan.output_size[1]
            if config.dry_run:
                byte_count = expected_size
            else:
                # Check preview before installing the resource pair so a preview
                # conflict cannot leave a newly installed resource behind.
                if plan.preview_path and plan.preview_path.exists() and not config.overwrite:
                    raise ConflictError(
                        f"preview already exists: {plan.preview_path}; use --overwrite"
                    )
                byte_count = write_pair_atomic(plan, mask, ron_text, config.overwrite)
                if plan.preview_path:
                    write_preview(plan.preview_path, mask, config.overwrite)

            result = ConversionResult(plan=plan, byte_count=byte_count)
            print_result(result, auto_requested=(config.mode == "auto"), dry_run=config.dry_run)
            converted += 1
            existing_ids[plan.resource_id] = plan.ron_path.expanduser().resolve()
        except ConflictError as exc:
            print(f"ERROR {source}: {exc}", file=sys.stderr)
            failed += 1
            conflict_seen = True
        except (ConversionError, OSError, ValueError) as exc:
            print(f"ERROR {source}: {exc}", file=sys.stderr)
            failed += 1

    print(f"converted: {converted}")
    print("skipped:   0")
    print(f"failed:    {failed}")

    if failed:
        return EXIT_CONFLICT if conflict_seen else EXIT_CONVERSION_ERROR
    return EXIT_OK


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return run(args)
    except ConflictError as exc:
        parser.exit(EXIT_CONFLICT, f"error: {exc}\n")
    except ConversionError as exc:
        parser.exit(EXIT_CONVERSION_ERROR, f"error: {exc}\n")
    return EXIT_OK


if __name__ == "__main__":
    raise SystemExit(main())

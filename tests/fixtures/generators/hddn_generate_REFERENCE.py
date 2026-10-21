"""Generate synthetic PDF traps without importing HDDN processing code."""

import argparse
import hashlib
import io
import json
import platform
import re
import subprocess
import zlib
from pathlib import Path

import PIL
import pypdf
import reportlab
from PIL import Image, ImageDraw, ImageFont
from pypdf import PdfReader, PdfWriter
from pypdf.annotations import Text
from pypdf.generic import (
    ArrayObject,
    BooleanObject,
    ByteStringObject,
    DecodedStreamObject,
    DictionaryObject,
    EncodedStreamObject,
    FloatObject,
    NameObject,
    NumberObject,
    RectangleObject,
    TextStringObject,
)
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen.canvas import Canvas

ROOT = Path(__file__).resolve().parents[2]
FONT = ROOT / "tests/pdf/assets/DejaVuSans.ttf"
TARGET = [68, 642, 360, 670]
RECORDS = []


def dictionary(items):
    return DictionaryObject({NameObject(key): value for key, value in items.items()})


def numbers(values):
    return ArrayObject([FloatObject(value) for value in values])


def stream(data, entries=None):
    result = DecodedStreamObject()
    result.set_data(data)
    result.update(dictionary(entries or {}))
    return result


def content(keep, secret=None):
    result = f"BT /F1 12 Tf 72 740 Td ({keep}) Tj ET\n"
    if secret:
        result += f"BT /F1 12 Tf 72 650 Td ({secret}) Tj ET\n"
    return result.encode("ascii")


def target(page, rect=TARGET, crop=(0, 0, 600, 800), rotation=0):
    x0, y0, x1, y1 = rect
    left, bottom, right, top = crop
    width, height = right - left, top - bottom
    a, b, c, d = x0 - left, top - y1, x1 - left, top - y0
    if rotation == 90:
        a, b, c, d = height - d, a, height - b, c
        width, height = height, width
    elif rotation == 180:
        a, b, c, d = width - c, height - d, width - a, height - b
    elif rotation == 270:
        a, b, c, d = b, width - c, d, width - a
        width, height = height, width
    return {
        "pageIndex": page,
        "pdfRectBottomLeft": list(rect),
        "cropBoxBottomLeft": list(crop),
        "rotation": rotation,
        "displaySize": [width, height],
        "box": {"x": a / width, "y": b / height,
                "width": (c - a) / width, "height": (d - b) / height},
    }


def base(name, count=1):
    writer = PdfWriter()
    writer.pdf_header = "%PDF-1.7"
    font = writer._add_object(dictionary({
        "/Type": NameObject("/Font"), "/Subtype": NameObject("/Type1"),
        "/BaseFont": NameObject("/Helvetica"),
    }))
    keep = "KEEP_PUBLIC_" + name.upper().replace("-", "_")
    secret = "SECRET_" + name.upper().replace("-", "_")
    for _ in range(count):
        page = writer.add_blank_page(600, 800)
        page[NameObject("/Resources")] = dictionary({"/Font": dictionary({"/F1": font})})
        page[NameObject("/Contents")] = writer._add_object(stream(content(keep, secret)))
    return writer, keep, secret


def serialize(writer):
    output = io.BytesIO()
    writer.write(output)
    return output.getvalue()


def save(output, name, writer=None, data=None, **expectations):
    data = data if data is not None else serialize(writer)
    (output / f"{name}.pdf").write_bytes(data)
    keep = "KEEP_PUBLIC_" + name.upper().replace("-", "_")
    secret = "SECRET_" + name.upper().replace("-", "_")
    record = {
        "id": name, "file": f"{name}.pdf", "sha256": hashlib.sha256(data).hexdigest(),
        "bytes": len(data), "pageCount": 1, "extractPages": [0],
        "sourceText": [keep, secret], "mustKeepText": [keep],
        "mustRemove": [secret], "targets": [target(0)],
        "inputPolicy": "load-completely", "checks": {},
    }
    record.update(expectations)
    RECORDS.append(record)


def image_object(label):
    image = Image.new("RGB", (200, 100), "white")
    draw = ImageDraw.Draw(image)
    font = ImageFont.truetype(str(FONT), 16)
    draw.text((4, 8), label, font=font, fill="black")
    draw.text((4, 65), "IMG_PUBLIC", font=font, fill="blue")
    raw = image.tobytes()
    return stream(raw, {
        "/Type": NameObject("/XObject"), "/Subtype": NameObject("/Image"),
        "/Width": NumberObject(200), "/Height": NumberObject(100),
        "/ColorSpace": NameObject("/DeviceRGB"), "/BitsPerComponent": NumberObject(8),
    })


def generate(output):
    RECORDS.clear()
    output.mkdir(parents=True, exist_ok=True)

    writer, _, _ = base("metadata-xmp")
    writer.add_metadata({"/Title": "SECRET_INFO_TITLE", "/Author": "SECRET_INFO_AUTHOR"})
    xmp = b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title><rdf:Alt><rdf:li xml:lang="x-default">SECRET_XMP_TITLE</rdf:li></rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta>'
    writer.root_object[NameObject("/Metadata")] = writer._add_object(stream(xmp, {
        "/Type": NameObject("/Metadata"), "/Subtype": NameObject("/XML"),
    }))
    save(output, "metadata-xmp", writer, checks={"metadata": {
        "/Title": "SECRET_INFO_TITLE", "/Author": "SECRET_INFO_AUTHOR"},
        "xmp": "SECRET_XMP_TITLE"}, mustRemove=["SECRET_METADATA_XMP", "SECRET_INFO_TITLE", "SECRET_INFO_AUTHOR", "SECRET_XMP_TITLE"])

    writer, _, secret = base("annotation-comment")
    writer.add_annotation(0, Text(rect=(420, 650, 440, 670), text="SECRET_COMMENT"))
    save(output, "annotation-comment", writer, checks={"annotation": "SECRET_COMMENT"}, mustRemove=[secret, "SECRET_COMMENT"])

    writer, _, secret = base("attachment")
    writer.add_attachment("SECRET_ATTACHMENT_NAME.txt", b"SECRET_ATTACHMENT_BODY")
    save(output, "attachment", writer, checks={"attachment": ["SECRET_ATTACHMENT_NAME.txt", "SECRET_ATTACHMENT_BODY"]}, mustRemove=[secret, "SECRET_ATTACHMENT_NAME", "SECRET_ATTACHMENT_BODY"])

    writer, _, secret = base("actions-js")
    action = writer._add_object(dictionary({"/S": NameObject("/JavaScript"), "/JS": TextStringObject('void("SECRET_JAVASCRIPT")')}))
    writer.root_object[NameObject("/OpenAction")] = action
    writer.add_js('void("SECRET_NAMED_JS")')
    writer.root_object["/Names"]["/JavaScript"]["/Names"][0] = TextStringObject("hddn-fixture-javascript")
    link = dictionary({"/Type": NameObject("/Annot"), "/Subtype": NameObject("/Link"),
        "/Rect": RectangleObject([420, 560, 500, 580]), "/A": dictionary({
            "/S": NameObject("/URI"), "/URI": TextStringObject("https://example.invalid/SECRET_URI_ACTION")})})
    writer.pages[0][NameObject("/Annots")] = ArrayObject([writer._add_object(link)])
    save(output, "actions-js", writer, checks={"javascript": "SECRET_JAVASCRIPT", "namedJavascript": "SECRET_NAMED_JS", "uri": "SECRET_URI_ACTION"}, mustRemove=[secret, "SECRET_JAVASCRIPT", "SECRET_NAMED_JS", "SECRET_URI_ACTION"])

    writer, _, secret = base("acroform")
    page = writer.pages[0]
    appearance = writer._add_object(stream(b"BT /F1 12 Tf 3 10 Td (SECRET_FORM_VALUE) Tj ET", {
        "/Type": NameObject("/XObject"), "/Subtype": NameObject("/Form"),
        "/BBox": RectangleObject([0, 0, 228, 30]), "/Resources": page["/Resources"],
    }))
    field = writer._add_object(dictionary({"/Type": NameObject("/Annot"),
        "/Subtype": NameObject("/Widget"), "/FT": NameObject("/Tx"),
        "/T": TextStringObject("SECRET_FORM_NAME"), "/V": TextStringObject("SECRET_FORM_VALUE"),
        "/DV": TextStringObject("SECRET_FORM_DEFAULT"), "/F": NumberObject(4),
        "/Rect": RectangleObject([72, 560, 300, 590]), "/P": page.indirect_reference,
        "/DA": TextStringObject("/F1 12 Tf 0 g"), "/AP": dictionary({"/N": appearance})}))
    page[NameObject("/Annots")] = ArrayObject([field])
    writer.root_object[NameObject("/AcroForm")] = writer._add_object(dictionary({
        "/Fields": ArrayObject([field]), "/DR": page["/Resources"],
        "/NeedAppearances": BooleanObject(False),
    }))
    save(output, "acroform", writer, checks={"form": {"name": "SECRET_FORM_NAME", "value": "SECRET_FORM_VALUE", "default": "SECRET_FORM_DEFAULT"}}, targets=[target(0), target(0, [70, 558, 310, 592])], mustRemove=[secret, "SECRET_FORM_NAME", "SECRET_FORM_VALUE", "SECRET_FORM_DEFAULT"])

    writer, keep, secret = base("optional-content-layer")
    layer = writer._add_object(dictionary({"/Type": NameObject("/OCG"), "/Name": TextStringObject("SECRET_LAYER_NAME")}))
    writer.root_object[NameObject("/OCProperties")] = dictionary({"/OCGs": ArrayObject([layer]), "/D": dictionary({"/OFF": ArrayObject([layer])})})
    writer.pages[0]["/Resources"][NameObject("/Properties")] = dictionary({"/Hidden": layer})
    writer.pages[0]["/Contents"].set_data(content(keep, secret) + b"/OC /Hidden BDC BT /F1 12 Tf 72 560 Td (SECRET_LAYER_BODY) Tj ET EMC\n")
    save(output, "optional-content-layer", writer, checks={"layer": ["SECRET_LAYER_NAME", "SECRET_LAYER_BODY"]}, mustRemove=[secret, "SECRET_LAYER_NAME", "SECRET_LAYER_BODY"])

    writer, keep, secret = base("hidden-ocr-text")
    writer.pages[0]["/Contents"].set_data(content(keep, secret) + b"BT /F1 12 Tf 3 Tr 72 560 Td (SECRET_INVISIBLE_OCR) Tj ET\n")
    save(output, "hidden-ocr-text", writer, checks={"invisibleText": "SECRET_INVISIBLE_OCR"}, mustRemove=[secret, "SECRET_INVISIBLE_OCR"])

    writer, _, secret = base("thumbnail")
    writer.pages[0][NameObject("/Thumb")] = writer._add_object(image_object("THUMB_SECRET"))
    save(output, "thumbnail", writer, checks={"thumbnail": True}, mustRemove=[secret], imageSecrets=[{"location": "/Thumb", "value": "THUMB_SECRET", "pixelRect": [0, 0, 200, 50]}])

    writer, _, secret = base("outline-bookmark")
    writer.add_outline_item("SECRET_OUTLINE_TITLE", 0)
    save(output, "outline-bookmark", writer, checks={"outline": "SECRET_OUTLINE_TITLE"}, mustRemove=[secret, "SECRET_OUTLINE_TITLE"])

    writer, keep, secret = base("actualtext")
    replacement = (b"\xfe\xff" + "SECRET_ACTUALTEXT".encode("utf-16-be")).hex().encode("ascii")
    writer.pages[0]["/Contents"].set_data(content(keep, secret) + b"/Span << /MCID 0 /ActualText <" + replacement + b"> >> BDC BT /F1 12 Tf 72 560 Td (PUBLIC_GLYPHS) Tj ET EMC\n")
    tree = dictionary({"/Type": NameObject("/StructTreeRoot")})
    tree_ref = writer._add_object(tree)
    element = writer._add_object(dictionary({"/Type": NameObject("/StructElem"), "/S": NameObject("/Span"),
        "/P": tree_ref, "/Pg": writer.pages[0].indirect_reference, "/K": NumberObject(0), "/Alt": TextStringObject("SECRET_STRUCTURE_ALT")}))
    tree[NameObject("/K")] = ArrayObject([element])
    tree[NameObject("/ParentTree")] = writer._add_object(dictionary({"/Nums": ArrayObject([NumberObject(0), ArrayObject([element])])}))
    writer.root_object[NameObject("/StructTreeRoot")] = tree_ref
    writer.root_object[NameObject("/MarkInfo")] = dictionary({"/Marked": BooleanObject(True)})
    writer.pages[0][NameObject("/StructParents")] = NumberObject(0)
    save(output, "actualtext", writer, checks={"actualText": "SECRET_ACTUALTEXT", "structureAlt": "SECRET_STRUCTURE_ALT"}, mustRemove=[secret, "SECRET_ACTUALTEXT", "SECRET_STRUCTURE_ALT"], mustKeepRenderedText=["PUBLIC_GLYPHS"], renderedTextRegions=[{"text": "PUBLIC_GLYPHS", "pageIndex": 0, "rect72dpi": [68, 224, 204, 248]}])

    writer, keep, secret = base("incremental-revision")
    original = serialize(writer)
    revision = PdfWriter(io.BytesIO(original), incremental=True)
    revision.pages[0][NameObject("/Contents")] = revision._add_object(stream(content(keep, "PUBLIC_REVISED")))
    save(output, "incremental-revision", data=serialize(revision), sourceText=[keep, "PUBLIC_REVISED"], mustKeepText=[keep, "PUBLIC_REVISED"], targets=[], checks={"previousRevision": secret})

    for name, subtype in [("shared-image", "image"), ("shared-vector", "vector")]:
        writer, keep, secret = base(name, 2)
        if subtype == "image":
            graphic = image_object("IMG_SECRET")
        else:
            segments = {"7": "abc", "3": "abcdg", "1": "bc", "9": "abcdfg"}
            lines = {"a": (0, 30, 16, 30), "b": (16, 30, 16, 15), "c": (16, 15, 16, 0), "d": (0, 0, 16, 0), "f": (0, 30, 0, 15), "g": (0, 15, 16, 15)}
            drawing = "0 G 2 w\n"
            for index, digit in enumerate("7319"):
                for segment in segments[digit]:
                    x0, y0, x1, y1 = lines[segment]
                    drawing += f"{10 + index * 35 + x0} {60 + y0} m {10 + index * 35 + x1} {60 + y1} l S\n"
            drawing += "0 0 1 rg 10 10 180 25 re f\n"
            graphic = stream(drawing.encode("ascii"), {"/Type": NameObject("/XObject"), "/Subtype": NameObject("/Form"), "/BBox": RectangleObject([0, 0, 200, 100]), "/Resources": DictionaryObject()})
        reference = writer._add_object(graphic)
        for index, page in enumerate(writer.pages):
            page["/Resources"][NameObject("/XObject")] = dictionary({"/Shared": reference})
            clip = b"72 500 240 60 re W n\n" if index else b""
            matrix = b"240 0 0 120" if subtype == "image" else b"1.2 0 0 1.2"
            page["/Contents"].set_data(content(keep + f"_P{index + 1}", secret if index == 0 else None) + b"q\n" + clip + matrix + b" 72 500 cm /Shared Do Q\n")
        save(output, name, writer, pageCount=2, extractPages=[0, 1], sourceText=[keep + "_P1", keep + "_P2", secret], mustKeepText=[keep + "_P1", keep + "_P2"], targets=[target(0), target(0, [70, 558, 314, 622])], checks={"sharedObject": subtype}, graphicSecret={"value": "IMG_SECRET" if subtype == "image" else "7319", "objectRectBottomLeft": [0, 50, 200, 100], "pixelRectTopLeft": [0, 0, 200, 50] if subtype == "image" else None, "publicOccurrencePage": 1, "publicValue": "IMG_PUBLIC" if subtype == "image" else "blue bar", "publicOnlyClipPdf": [72, 500, 312, 560]})

    writer, keep, secret = base("rotation-crop", 4)
    crop = [20, 30, 580, 770]
    for index, page in enumerate(writer.pages):
        page.cropbox = RectangleObject(crop)
        page[NameObject("/Rotate")] = NumberObject(index * 90)
    save(output, "rotation-crop", writer, pageCount=4, extractPages=[0, 1, 2, 3], targets=[target(index, crop=crop, rotation=index * 90) for index in range(4)], checks={"rotations": [0, 90, 180, 270], "crop": crop})

    pdfmetrics.registerFont(TTFont("FixtureUnicode", str(FONT)))
    buffer = io.BytesIO()
    canvas = Canvas(buffer, pagesize=(600, 800), invariant=1, pageCompression=0)
    canvas.setFont("FixtureUnicode", 20)
    canvas.drawString(72, 740, "KEEP_PUBLIC_UNICODE_GLYPH_WIDTHS")
    prefix = "A😀 WiWi ﬁ Œ "
    canvas.drawString(72, 650, prefix + "SECRET_UNICODE")
    canvas.save()
    unicode_writer = PdfWriter(clone_from=PdfReader(io.BytesIO(buffer.getvalue())))
    for font in unicode_writer.pages[0]["/Resources"]["/Font"].values():
        cmap = font.get_object().get("/ToUnicode")
        if cmap:
            cmap = cmap.get_object()
            # ReportLab emits non-BMP targets as scalar hex, not UTF-16BE pairs.
            cmap.set_data(re.sub(rb"<([0-9A-Fa-f]{5,6})>", lambda match: b"<" + chr(int(match[1], 16)).encode("utf-16-be").hex().encode("ascii") + b">", cmap.get_data()))
    start = 72 + pdfmetrics.stringWidth(prefix, "FixtureUnicode", 20)
    end = start + pdfmetrics.stringWidth("SECRET_UNICODE", "FixtureUnicode", 20)
    save(output, "unicode-glyph-widths", data=serialize(unicode_writer), sourceText=["KEEP_PUBLIC_UNICODE_GLYPH_WIDTHS", prefix + "SECRET_UNICODE"], mustRemove=["SECRET_UNICODE"], targets=[target(0, [start - 1, 644, end + 1, 671])], checks={"unicode": {"prefix": prefix, "secret": "SECRET_UNICODE", "offsetScope": "body text run", "codePointOffset": len(prefix), "utf16Offset": len(prefix.encode("utf-16-le")) // 2, "font": "FixtureUnicode", "proportional": True}})

    writer, _, _ = base("encrypted")
    identifier = hashlib.sha256(b"HDDN_SYNTHETIC_ENCRYPTION_FIXTURE").digest()[:16]
    writer._ID = ArrayObject([ByteStringObject(identifier), ByteStringObject(identifier)])
    writer.encrypt("hddn-fixture-user", "hddn-fixture-owner", algorithm="RC4-128")
    save(output, "encrypted", writer, password="hddn-fixture-user", ownerPassword="hddn-fixture-owner", wrongPassword="hddn-fixture-wrong", inputPolicy="require-supplied-password", checks={"encrypted": True})

    writer, _, _ = base("malformed-truncated")
    data = serialize(writer)
    save(output, "malformed-truncated", data=data[:data.index(b"xref\n")], pageCount=None, sourceText=[], mustKeepText=[], mustRemove=[], targets=[], inputPolicy="reject", doNotRender=True, checks={"truncated": True})

    writer, _, _ = base("malformed-xref")
    data = serialize(writer)
    position = data.rindex(b"startxref\n") + len(b"startxref\n")
    end = data.index(b"\n", position)
    data = data[:position] + b"1" + data[end:]
    save(output, "malformed-xref", data=data, inputPolicy="reject-or-complete-repair", checks={"badXref": True})

    writer, keep, secret = base("many-pages-1001", 1001)
    for index, page in enumerate(writer.pages):
        page["/Contents"].set_data(content(keep + f"_P{index + 1}", secret if index == 1000 else None))
    save(output, "many-pages-1001", writer, pageCount=1001, extractPages=[0, 1000], sourceText=[keep + "_P1", keep + "_P1001", secret], mustKeepText=[keep + "_P1", keep + "_P1001"], targets=[target(1000)], inputPolicy="bounded-load-or-explicit-capability-rejection", doNotRender=True, checks={"manyPages": 1001})

    writer, _, _ = base("oversized-image")
    graphic = stream(b"\x00\x00\x00", {"/Type": NameObject("/XObject"), "/Subtype": NameObject("/Image"), "/Width": NumberObject(100000), "/Height": NumberObject(100000), "/ColorSpace": NameObject("/DeviceRGB"), "/BitsPerComponent": NumberObject(8)})
    writer.pages[0]["/Resources"][NameObject("/XObject")] = dictionary({"/Huge": writer._add_object(graphic)})
    writer.pages[0]["/Contents"].set_data(writer.pages[0]["/Contents"].get_data() + b"q 10 0 0 10 72 500 cm /Huge Do Q\n")
    save(output, "oversized-image", writer, doNotRender=True, checks={"oversizedImage": [100000, 100000, 3]}, inputPolicy="reject-before-unbounded-image-allocation")

    writer, _, _ = base("compressed-stream")
    bomb = EncodedStreamObject()
    bomb._data = zlib.compress(b"q Q\n" * (2 * 1024 * 1024), 9)
    bomb[NameObject("/Filter")] = NameObject("/FlateDecode")
    first = writer.pages[0].raw_get("/Contents")
    writer.pages[0][NameObject("/Contents")] = ArrayObject([first, writer._add_object(bomb)])
    save(output, "compressed-stream", writer, doNotRender=True, doNotExtract=True, checks={"compressedStream": {"expandedBytes": 8 * 1024 * 1024, "verificationLimit": 65536}}, inputPolicy="bounded-decode-or-explicit-resource-rejection")

    manifest = {
        "schemaVersion": 1,
        "purpose": "Synthetic source oracles, not evidence that HDDN exports are safe",
        "generator": "scripts/pdf-fixtures/generate.py",
        "versions": {"Python": platform.python_version(), "pypdf": pypdf.__version__, "reportlab": reportlab.Version, "Pillow": PIL.__version__},
        "font": {"file": "tests/pdf/assets/DejaVuSans.ttf", "sha256": hashlib.sha256(FONT.read_bytes()).hexdigest(), "license": "tests/pdf/assets/DejaVu-LICENSE.txt", "licenseSha256": hashlib.sha256((FONT.parent / "DejaVu-LICENSE.txt").read_bytes()).hexdigest()},
        "coordinates": "Normalized top-left display coordinates after CropBox and clockwise Rotate; pdfRectBottomLeft is authored PDF user space",
        "fixtures": RECORDS,
    }
    manifest_path = output / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
    subprocess.run(["pnpm", "exec", "prettier", str(manifest_path), "--config", str(ROOT / ".prettierrc.mjs"), "--write"], cwd=ROOT, check=True, capture_output=True)
    print(f"Generated {len(RECORDS)} synthetic PDFs and manifest in {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "tests/pdf/fixtures")
    generate(parser.parse_args().output.resolve())

# Deterministic image-heavy PDF: 6 A4 pages, each one full-page 2480x1754 photo-like JPEG (dev-only PyMuPDF+numpy).
# Usage: make-imageheavy.py OUT.pdf
import sys, fitz, numpy as np
W, H = 2480, 1754
doc = fitz.open()
for p in range(6):
    rng = np.random.default_rng(1000 + p)
    y, x = np.mgrid[0:H, 0:W].astype(np.float32)
    img = np.empty((H, W, 3), np.float32)
    for c in range(3):
        ph = rng.uniform(0, 6.28, 3)
        img[..., c] = 128 + 60 * np.sin(x / (180 + 60 * c) + ph[0]) + 50 * np.cos(y / (140 + 40 * c) + ph[1]) + 25 * np.sin((x + y) / 37 + ph[2])
    img += rng.normal(0, 9, img.shape).astype(np.float32)
    pix = fitz.Pixmap(fitz.csRGB, W, H, np.clip(img, 0, 255).astype(np.uint8).tobytes(), False)
    page = doc.new_page(width=595, height=842)
    page.insert_image(fitz.Rect(0, 0, 595, 842), stream=pix.tobytes("jpeg", jpg_quality=85), keep_proportion=False)
    page.insert_text((56, 64), f"Image page {p + 1}", fontsize=18, fontname="helv", color=(1, 1, 1))
doc.set_metadata({"creationDate": "D:20260101000000Z", "modDate": "D:20260101000000Z"})
doc.save(sys.argv[1], no_new_id=True, garbage=0)

# Deterministic 30-page native-text PDF (dev-only PyMuPDF; the manifest has no ~30-page text fixture). Usage: make-native30.py OUT.pdf
import sys, fitz
words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango".split()
doc = fitz.open()
for p in range(30):
    page = doc.new_page(width=595, height=842)
    page.insert_text((56, 64), f"Bench page {p + 1}", fontsize=18, fontname="helv")
    for line in range(40):
        text = " ".join(words[(p * 7 + line * 3 + i) % len(words)] for i in range(14))
        page.insert_text((56, 100 + line * 17), text, fontsize=10, fontname="helv")
doc.set_metadata({"creationDate": "D:20260101000000Z", "modDate": "D:20260101000000Z"})
doc.save(sys.argv[1], no_new_id=True)

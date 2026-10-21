//! Writes the browser-smoke fixture: `cargo run -p foliark-engine --example smoke_pdf > tests/smoke/one-page.pdf`.
//! One 200x100 pt page: black Helvetica text and a red rectangle at x 120..160, y 10..40 (PDF space).
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};
use std::io::Write;

fn main() {
    let (cat, tree, page, font, content) = (
        Ref::new(1),
        Ref::new(2),
        Ref::new(3),
        Ref::new(4),
        Ref::new(5),
    );
    let mut pdf = Pdf::new();
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.parent(tree)
        .media_box(Rect::new(0.0, 0.0, 200.0, 100.0))
        .contents(content);
    p.resources().fonts().pair(Name(b"F1"), font);
    p.finish();
    pdf.type1_font(font).base_font(Name(b"Helvetica"));
    let mut c = Content::new();
    c.begin_text()
        .set_font(Name(b"F1"), 18.0)
        .next_line(10.0, 60.0)
        .show(Str(b"Hello Foliark"))
        .end_text();
    c.set_fill_rgb(1.0, 0.0, 0.0)
        .rect(120.0, 10.0, 40.0, 30.0)
        .fill_nonzero();
    pdf.stream(content, &c.finish());
    std::io::stdout().write_all(&pdf.finish()).unwrap();
}

fn main() {
    let r = foliark_spike::run(foliark_spike::build_pdf(false)).unwrap();
    println!("{r:#?}");
}

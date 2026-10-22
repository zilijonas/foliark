// esbuild's `file` loader turns the .wasm into its URL.
declare module "@zilijonas/foliark/wasm" {
  const url: string;
  export default url;
}

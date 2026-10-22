// esbuild's `file` loader turns the .wasm into its URL.
declare module "@foliark/sdk/wasm" {
  const url: string;
  export default url;
}

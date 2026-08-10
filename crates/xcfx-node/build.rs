use std::env;

fn main() {
  let target = env::var("TARGET").expect("Cargo must provide TARGET to build scripts");

  println!("cargo::rerun-if-changed=build.rs");
  println!("cargo::rustc-env=XCFX_NODE_BUILD_TARGET={target}");
}

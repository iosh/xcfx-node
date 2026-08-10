use napi_derive::napi;
use xcfx_node::BuildInfo;

#[napi(object)]
pub struct NativeBuildInfo {
  pub version: String,
  pub target: String,
}

impl From<BuildInfo> for NativeBuildInfo {
  fn from(value: BuildInfo) -> Self {
    Self {
      version: value.version.to_owned(),
      target: value.target.to_owned(),
    }
  }
}

#[napi]
pub fn get_native_build_info() -> NativeBuildInfo {
  xcfx_node::build_info().into()
}

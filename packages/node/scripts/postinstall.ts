// get the current platform and architecture
const { platform, arch } = process;

// Currently only limited platforms are supported.
const ALL_SUPPORT_PLATFORMS: Partial<
  Record<typeof platform, Partial<Record<typeof arch, string>>>
> = {
  win32: {
    x64: "@xcfx/node-win32-x64",
  },
  linux: {
    x64: "@xcfx/node-linux-x64",
  },
  darwin: {
    arm64: "@xcfx/node-darwin-arm64",
  },
};

function main() {
  if (!ALL_SUPPORT_PLATFORMS[platform]) {
    return console.warn(
      `@xcfx/node is not supported the current platform: ${platform} yet. You can submit an issue: https://github.com/iosh/xcfx-node/issues/new`,
    );
  }

  if (!ALL_SUPPORT_PLATFORMS[platform][arch]) {
    return console.warn(
      `@xcfx/node is not supported the current architecture: ${arch} yet. You can submit an issue: https://github.com/iosh/xcfx-node/issues/new`,
    );
  }

  const BIN_NAME = ALL_SUPPORT_PLATFORMS[platform][arch];

  try {
    require.resolve(BIN_NAME);
  } catch {
    return console.warn(
      `@xcfx/node postinstall script is failed to resolve the binary conflux rust package: ${BIN_NAME}`,
    );
  }
}

main();

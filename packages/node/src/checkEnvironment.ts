const { platform, arch } = process;

// Currently only limited platforms are supported.
const ALL_SUPPORT_PLATFORMS: Partial<
  Record<typeof platform, Partial<Record<typeof arch, string>>>
> = {
  linux: {
    x64: "@xcfx/node-linux-x64",
  },
  darwin: {
    arm64: "@xcfx/node-darwin-arm64",
  },
};

type CheckEnvironmentReturnType =
  | {
      supportPlatform: true;
      binPath: string;
    }
  | {
      supportPlatform: false;
      message: string;
    };

/**
 * Check the current platform and architecture is supported
 * @returns { CheckEnvironmentReturnType }
 */
function checkEnvironment(): CheckEnvironmentReturnType {
  if (!ALL_SUPPORT_PLATFORMS[platform]) {
    return {
      supportPlatform: false,
      message: `@xcfx/node is not supported the current platform: ${platform} yet. You can submit an issue: https://github.com/iosh/xcfx-node/issues/new`,
    };
  }

  if (!ALL_SUPPORT_PLATFORMS[platform][arch]) {
    return {
      supportPlatform: false,
      message: `@xcfx/node is not supported the current architecture: ${arch} yet. You can submit an issue: https://github.com/iosh/xcfx-node/issues/new`,
    };
  }

  const BIN_NAME = `${ALL_SUPPORT_PLATFORMS[platform][arch]}/conflux`;

  return {
    supportPlatform: true,
    binPath: require.resolve(BIN_NAME),
  };
}

export default checkEnvironment;

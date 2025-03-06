# @xcfx/node

## 0.5.0

### Minor Changes

- 63b8773: Refactored Conflux node now runs via child_process
- cbba2df: Added support for configuration via external config file
- eb8e6f4: Added support for confluxDataDir to configure the data storage location

### Patch Changes

- 4c46a34: Added poll_lifetime_in_seconds default value
- bbea078: Added exit-hook package to handle process exit
- 061f568: Remove the log_file config
- 0c7b18f: Added `cip112_transition_height` parameter to the configuration"
- 9372280: Set `posReferenceEnableHeight` default value to 0, which enables POS by node start

## 0.4.0

### Minor Changes

- 5424c0a: Updated conflux-rust 2.4.1

## 0.3.2

### Patch Changes

- 8b0f478: Added pollLifetimeInSeconds and getLogsFilterMaxLimit to config

## 0.3.1

### Patch Changes

- e18c3e2: Added config files update cive version

## 0.3.0

### Minor Changes

- 34f5028: Update version Number(no changes)

## 0.2.0

### Minor Changes

- c1ab144: Use napi-rs to create a conflux-rust binding

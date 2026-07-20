// Application version metadata, surfaced under About / Diagnostics.
export const APP_VERSION = '0.9.1';
export const BUILD_MODE = (import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV ? 'development' : 'production';

/**
 * Re-exports the shared Slurp interfaces used by the runtime.
 *
 * The shared package lives outside this package's tree (../../shared/interfaces.ts).
 * These are stable, framework-level contracts - do not modify them here; edit
 * the canonical source in shared/interfaces.ts instead.
 */
export type {
  SlurpError,
  HmrMessage,
  FetchState,
  FetchErrorState,
  PaginatedFetchState,
  InfiniteFetchState,
} from '../../shared/interfaces'

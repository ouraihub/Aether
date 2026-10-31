/** Structural subsets: the adapter needs no Cloudflare ambient globals. */
export interface D1Statement {
  bind(...values: (string | number | null)[]): D1Statement
  first(): Promise<unknown>
  all(): Promise<{
    readonly results?: readonly unknown[]
    readonly success: boolean
  }>
}
export interface D1Database {
  prepare(sql: string): D1Statement
}
export interface R2Object {
  readonly size: number
  readonly httpMetadata?: { readonly contentType?: string }
  arrayBuffer(): Promise<ArrayBuffer>
}
export interface R2Bucket {
  put(
    key: string,
    value: ArrayBuffer | Uint8Array,
    options?: {
      readonly httpMetadata?: { readonly contentType?: string }
      readonly onlyIf?: { readonly etagDoesNotMatch: string }
    },
  ): Promise<unknown>
  get(key: string): Promise<R2Object | null>
  delete(key: string): Promise<void>
}
export interface DurableObjectStorage {
  get<T = unknown>(key: string): Promise<T | undefined>
  put<T>(key: string, value: T): Promise<void>
  delete(key: string): Promise<boolean>
  setAlarm(scheduledTime: number | Date): Promise<void>
  getAlarm(): Promise<number | null>
  deleteAlarm(): Promise<void>
}
export interface DurableObjectState {
  readonly storage: DurableObjectStorage
  blockConcurrencyWhile<T>(callback: () => Promise<T>): Promise<T>
  waitUntil(promise: Promise<unknown>): void
}
export interface DurableObjectStub {
  fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response>
}
export interface DurableObjectNamespace {
  idFromName(name: string): unknown
  get(id: unknown): DurableObjectStub
}
export interface FetchBinding {
  fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response>
}

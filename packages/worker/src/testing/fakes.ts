// In-memory stand-ins for the Durable Object and document storage ports, shared by unit tests and
// the scenario runner. They hold everything in maps and clone on every read and write, so callers
// never alias stored values.
import { KitFailure } from '@ouraihub/aether-core'
import type { DurableObjectState, DurableObjectStorage } from '../cloudflare.js'
import type { Actor } from '../context.js'
import type { DocumentStore, StoredDocument } from '../storage.js'

/** Durable Object storage; `writes` records every stored value in order, `alarm` the scheduled alarm. */
export class MemoryStorage implements DurableObjectStorage {
  readonly records = new Map<string, unknown>()
  readonly writes: unknown[] = []
  alarm: number | null = null
  async get<T = unknown>(key: string): Promise<T | undefined> {
    return structuredClone(this.records.get(key)) as T | undefined
  }
  async put<T>(key: string, value: T): Promise<void> {
    this.records.set(key, structuredClone(value))
    this.writes.push(structuredClone(value))
  }
  async delete(key: string): Promise<boolean> {
    return this.records.delete(key)
  }
  async setAlarm(value: number | Date): Promise<void> {
    this.alarm = Number(value)
  }
  async getAlarm(): Promise<number | null> {
    return this.alarm
  }
  async deleteAlarm(): Promise<void> {
    this.alarm = null
  }
}

/** Durable Object state; `drain()` resolves once every `waitUntil` task (including ones they add) settled. */
export class MemoryState implements DurableObjectState {
  readonly storage: MemoryStorage
  readonly tasks = new Set<Promise<unknown>>()
  constructor(storage = new MemoryStorage()) {
    this.storage = storage
  }
  blockConcurrencyWhile<T>(callback: () => Promise<T>): Promise<T> {
    return callback()
  }
  waitUntil(promise: Promise<unknown>): void {
    this.tasks.add(promise)
    void promise.finally(() => this.tasks.delete(promise)).catch(() => undefined)
  }
  async drain(): Promise<void> {
    while (this.tasks.size) await Promise.all([...this.tasks])
  }
}

/** Owner-scoped document store with revision compare-and-set; `failConversations` fails conversation writes. */
export class MemoryDocuments implements DocumentStore {
  readonly documents = new Map<string, StoredDocument>()
  failConversations = false
  private id(owner: Actor, collection: string, key: string): string {
    return JSON.stringify([owner.applicationId, owner.tenantId, owner.subjectId, collection, key])
  }
  async get(owner: Actor, collection: string, key: string): Promise<StoredDocument | null> {
    return structuredClone(this.documents.get(this.id(owner, collection, key)) ?? null)
  }
  async list(owner: Actor, collection: string): Promise<readonly StoredDocument[]> {
    const prefix = JSON.stringify([owner.applicationId, owner.tenantId, owner.subjectId, collection]).slice(0, -1)
    return [...this.documents.entries()]
      .filter(([key]) => key.startsWith(prefix))
      .map(([, value]) => structuredClone(value))
  }
  async compareAndSet(
    owner: Actor,
    collection: string,
    key: string,
    expected: number | null,
    value: unknown,
  ): Promise<StoredDocument> {
    if (collection === 'conversations' && this.failConversations)
      throw new KitFailure('internal', 'secret-token database failure')
    const id = this.id(owner, collection, key),
      old = this.documents.get(id)
    if ((old?.revision ?? null) !== expected) throw new KitFailure('conflict', 'Concurrent update')
    const saved = { key, revision: (old?.revision ?? 0) + 1, value: structuredClone(value) }
    this.documents.set(id, saved)
    return structuredClone(saved)
  }
  async remove(owner: Actor, collection: string, key: string, expected: number): Promise<void> {
    const id = this.id(owner, collection, key)
    if (this.documents.get(id)?.revision !== expected) throw new KitFailure('conflict', 'Concurrent update')
    this.documents.delete(id)
  }
}

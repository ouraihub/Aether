import type {
  ModelRef,
  RunAccepted,
  RunEvent,
  Source,
} from '@ouraihub/aether-contracts'
import type { CallOptions } from './contract'

export interface SkillCandidate {
  name: string
  description: string
  url: string
  origin: string
}
export interface SystemSkillCandidate {
  name: string
  path: string
  origin: string
}
export interface PreparedSkill {
  source: Source
  origin: string
  ignored?: string[]
}
/** Multi-turn skill workspace; install only through preview plus digest. */
export interface SkillAuthoringPort {
  create(options?: CallOptions): Promise<string>
  send(
    sessionId: string,
    input: string,
    model: ModelRef,
    onEvent: (event: RunEvent) => void,
    options?: CallOptions,
  ): Promise<RunAccepted>
  approve(
    sessionId: string,
    runId: string,
    callId: string,
    allow: boolean,
  ): Promise<void>
  cancel(sessionId: string, runId: string): Promise<void>
  prepare(sessionId: string, options?: CallOptions): Promise<PreparedSkill>
  close(sessionId: string): Promise<void>
}

/** Optional acquisition capabilities; absent desktop actions stay online. */
export interface SkillSourcesPort {
  search(query: string, options?: CallOptions): Promise<SkillCandidate[]>
  prepare(url: string, options?: CallOptions): Promise<PreparedSkill>
  discover(options?: CallOptions): Promise<SystemSkillCandidate[]>
  prepareSystem(path: string, options?: CallOptions): Promise<PreparedSkill>
  pickDirectory(options?: CallOptions): Promise<PreparedSkill | null>
  pickSearchDirectory(
    options?: CallOptions,
  ): Promise<SystemSkillCandidate[] | null>
  authoring?: SkillAuthoringPort
}

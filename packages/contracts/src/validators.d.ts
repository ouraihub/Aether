import type { ValidateFunction } from 'ajv'
import type { Ack } from './generated/Ack'
import type { Approval } from './generated/Approval'
import type { ContextRef } from './generated/ContextRef'
import type { ContextSource } from './generated/ContextSource'
import type { ContextUsage } from './generated/ContextUsage'
import type { Conversation } from './generated/Conversation'
import type { ConversationSummary } from './generated/ConversationSummary'
import type { Cost } from './generated/Cost'
import type { Currency } from './generated/Currency'
import type { Details } from './generated/Details'
import type { EndStatus } from './generated/EndStatus'
import type { Endpoint } from './generated/Endpoint'
import type { EndpointKey } from './generated/EndpointKey'
import type { EndpointKeySave } from './generated/EndpointKeySave'
import type { EndpointSave } from './generated/EndpointSave'
import type { ImagePart } from './generated/ImagePart'
import type { ImageRef } from './generated/ImageRef'
import type { IngestBatch } from './generated/IngestBatch'
import type { IngestEntry } from './generated/IngestEntry'
import type { IngestLevel } from './generated/IngestLevel'
import type { IngestReceipt } from './generated/IngestReceipt'
import type { IssuedToken } from './generated/IssuedToken'
import type { KeyStrategy } from './generated/KeyStrategy'
import type { KitError } from './generated/KitError'
import type { KitErrorCode } from './generated/KitErrorCode'
import type { LogEntry } from './generated/LogEntry'
import type { LogLevel } from './generated/LogLevel'
import type { LogStatus } from './generated/LogStatus'
import type { ManualSave } from './generated/ManualSave'
import type { McpTransport } from './generated/McpTransport'
import type { Model } from './generated/Model'
import type { ModelFilter } from './generated/ModelFilter'
import type { ModelOption } from './generated/ModelOption'
import type { ModelPricing } from './generated/ModelPricing'
import type { ModelRef } from './generated/ModelRef'
import type { ModelSave } from './generated/ModelSave'
import type { ModelType } from './generated/ModelType'
import type { PermissionMode } from './generated/PermissionMode'
import type { Preview } from './generated/Preview'
import type { PreviewAction } from './generated/PreviewAction'
import type { PreviewCurrent } from './generated/PreviewCurrent'
import type { PricingTier } from './generated/PricingTier'
import type { ProbeError } from './generated/ProbeError'
import type { ProbeErrorKind } from './generated/ProbeErrorKind'
import type { ProbeResult } from './generated/ProbeResult'
import type { ProbeTarget } from './generated/ProbeTarget'
import type { ProbedModel } from './generated/ProbedModel'
import type { Protocol } from './generated/Protocol'
import type { Provider } from './generated/Provider'
import type { ProviderSave } from './generated/ProviderSave'
import type { ReasoningConfig } from './generated/ReasoningConfig'
import type { ReasoningDialect } from './generated/ReasoningDialect'
import type { ReasoningEffort } from './generated/ReasoningEffort'
import type { RunAccepted } from './generated/RunAccepted'
import type { RunEvent } from './generated/RunEvent'
import type { RunRequest } from './generated/RunRequest'
import type { RunSnapshot } from './generated/RunSnapshot'
import type { RunStatus } from './generated/RunStatus'
import type { ScopeState } from './generated/ScopeState'
import type { SearchCap } from './search'
import type { SearchHit } from './generated/SearchHit'
import type { SearchKey } from './search'
import type { SearchPage } from './search'
import type { SearchPreferences } from './search'
import type { SearchProvider } from './search'
import type { SearchResult } from './search'
import type { SearchSave } from './search'
import type { SearchSettingsDto } from './search'
import type { ServerDto } from './generated/ServerDto'
import type { ServerSave } from './generated/ServerSave'
import type { ServerSecret } from './generated/ServerSecret'
import type { SkillDto } from './generated/SkillDto'
import type { SkillFileKind } from './generated/SkillFileKind'
import type { SkillFileMeta } from './generated/SkillFileMeta'
import type { SkillMeta } from './generated/SkillMeta'
import type { SkillOrigin } from './generated/SkillOrigin'
import type { SkillRef } from './generated/SkillRef'
import type { Source } from './generated/Source'
import type { Status } from './generated/Status'
import type { TokenRecord } from './generated/TokenRecord'
import type { TokenUsage } from './generated/TokenUsage'
import type { ToolDto } from './generated/ToolDto'
import type { ToolEffect } from './generated/ToolEffect'
import type { ToolOutcome } from './generated/ToolOutcome'
import type { ToolSourceKind } from './generated/ToolSourceKind'
import type { Turn } from './generated/Turn'
import type { TurnStatus } from './generated/TurnStatus'
import type { TypeSource } from './generated/TypeSource'
export const validateAck: ValidateFunction<Ack>
export const validateApproval: ValidateFunction<Approval>
export const validateContextRef: ValidateFunction<ContextRef>
export const validateContextSource: ValidateFunction<ContextSource>
export const validateContextUsage: ValidateFunction<ContextUsage>
export const validateConversation: ValidateFunction<Conversation>
export const validateConversationSummary: ValidateFunction<ConversationSummary>
export const validateCost: ValidateFunction<Cost>
export const validateCurrency: ValidateFunction<Currency>
export const validateDetails: ValidateFunction<Details>
export const validateEndStatus: ValidateFunction<EndStatus>
export const validateEndpoint: ValidateFunction<Endpoint>
export const validateEndpointKey: ValidateFunction<EndpointKey>
export const validateEndpointKeySave: ValidateFunction<EndpointKeySave>
export const validateEndpointSave: ValidateFunction<EndpointSave>
export const validateImagePart: ValidateFunction<ImagePart>
export const validateImageRef: ValidateFunction<ImageRef>
export const validateIngestBatch: ValidateFunction<IngestBatch>
export const validateIngestEntry: ValidateFunction<IngestEntry>
export const validateIngestLevel: ValidateFunction<IngestLevel>
export const validateIngestReceipt: ValidateFunction<IngestReceipt>
export const validateIssuedToken: ValidateFunction<IssuedToken>
export const validateKeyStrategy: ValidateFunction<KeyStrategy>
export const validateKitError: ValidateFunction<KitError>
export const validateKitErrorCode: ValidateFunction<KitErrorCode>
export const validateLogEntry: ValidateFunction<LogEntry>
export const validateLogLevel: ValidateFunction<LogLevel>
export const validateLogStatus: ValidateFunction<LogStatus>
export const validateManualSave: ValidateFunction<ManualSave>
export const validateMcpTransport: ValidateFunction<McpTransport>
export const validateModel: ValidateFunction<Model>
export const validateModelFilter: ValidateFunction<ModelFilter>
export const validateModelOption: ValidateFunction<ModelOption>
export const validateModelPricing: ValidateFunction<ModelPricing>
export const validateModelRef: ValidateFunction<ModelRef>
export const validateModelSave: ValidateFunction<ModelSave>
export const validateModelType: ValidateFunction<ModelType>
export const validatePermissionMode: ValidateFunction<PermissionMode>
export const validatePreview: ValidateFunction<Preview>
export const validatePreviewAction: ValidateFunction<PreviewAction>
export const validatePreviewCurrent: ValidateFunction<PreviewCurrent>
export const validatePricingTier: ValidateFunction<PricingTier>
export const validateProbeError: ValidateFunction<ProbeError>
export const validateProbeErrorKind: ValidateFunction<ProbeErrorKind>
export const validateProbeResult: ValidateFunction<ProbeResult>
export const validateProbeTarget: ValidateFunction<ProbeTarget>
export const validateProbedModel: ValidateFunction<ProbedModel>
export const validateProtocol: ValidateFunction<Protocol>
export const validateProvider: ValidateFunction<Provider>
export const validateProviderSave: ValidateFunction<ProviderSave>
export const validateReasoningConfig: ValidateFunction<ReasoningConfig>
export const validateReasoningDialect: ValidateFunction<ReasoningDialect>
export const validateReasoningEffort: ValidateFunction<ReasoningEffort>
export const validateRunAccepted: ValidateFunction<RunAccepted>
export const validateRunEvent: ValidateFunction<RunEvent>
export const validateRunRequest: ValidateFunction<RunRequest>
export const validateRunSnapshot: ValidateFunction<RunSnapshot>
export const validateRunStatus: ValidateFunction<RunStatus>
export const validateScopeState: ValidateFunction<ScopeState>
export const validateSearchCap: ValidateFunction<SearchCap>
export const validateSearchHit: ValidateFunction<SearchHit>
export const validateSearchKey: ValidateFunction<SearchKey>
export const validateSearchPage: ValidateFunction<SearchPage>
export const validateSearchPreferences: ValidateFunction<SearchPreferences>
export const validateSearchProvider: ValidateFunction<SearchProvider>
export const validateSearchResult: ValidateFunction<SearchResult>
export const validateSearchSave: ValidateFunction<SearchSave>
export const validateSearchSettingsDto: ValidateFunction<SearchSettingsDto>
export const validateServerDto: ValidateFunction<ServerDto>
export const validateServerSave: ValidateFunction<ServerSave>
export const validateServerSecret: ValidateFunction<ServerSecret>
export const validateSkillDto: ValidateFunction<SkillDto>
export const validateSkillFileKind: ValidateFunction<SkillFileKind>
export const validateSkillFileMeta: ValidateFunction<SkillFileMeta>
export const validateSkillMeta: ValidateFunction<SkillMeta>
export const validateSkillOrigin: ValidateFunction<SkillOrigin>
export const validateSkillRef: ValidateFunction<SkillRef>
export const validateSource: ValidateFunction<Source>
export const validateStatus: ValidateFunction<Status>
export const validateTokenRecord: ValidateFunction<TokenRecord>
export const validateTokenUsage: ValidateFunction<TokenUsage>
export const validateToolDto: ValidateFunction<ToolDto>
export const validateToolEffect: ValidateFunction<ToolEffect>
export const validateToolOutcome: ValidateFunction<ToolOutcome>
export const validateToolSourceKind: ValidateFunction<ToolSourceKind>
export const validateTurn: ValidateFunction<Turn>
export const validateTurnStatus: ValidateFunction<TurnStatus>
export const validateTypeSource: ValidateFunction<TypeSource>

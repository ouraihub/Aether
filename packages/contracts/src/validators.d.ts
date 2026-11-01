import type { ValidateFunction } from 'ajv'
import type { Ack } from './generated/Ack.js'
import type { Approval } from './generated/Approval.js'
import type { ContextRef } from './generated/ContextRef.js'
import type { ContextSource } from './generated/ContextSource.js'
import type { ContextUsage } from './generated/ContextUsage.js'
import type { Conversation } from './generated/Conversation.js'
import type { ConversationSummary } from './generated/ConversationSummary.js'
import type { Cost } from './generated/Cost.js'
import type { Currency } from './generated/Currency.js'
import type { Details } from './generated/Details.js'
import type { EndStatus } from './generated/EndStatus.js'
import type { Endpoint } from './generated/Endpoint.js'
import type { EndpointKey } from './generated/EndpointKey.js'
import type { EndpointKeySave } from './generated/EndpointKeySave.js'
import type { EndpointSave } from './generated/EndpointSave.js'
import type { ImagePart } from './generated/ImagePart.js'
import type { ImageRef } from './generated/ImageRef.js'
import type { IngestBatch } from './generated/IngestBatch.js'
import type { IngestEntry } from './generated/IngestEntry.js'
import type { IngestLevel } from './generated/IngestLevel.js'
import type { IngestReceipt } from './generated/IngestReceipt.js'
import type { IssuedToken } from './generated/IssuedToken.js'
import type { KeyStrategy } from './generated/KeyStrategy.js'
import type { KitError } from './generated/KitError.js'
import type { KitErrorCode } from './generated/KitErrorCode.js'
import type { LogEntry } from './generated/LogEntry.js'
import type { LogLevel } from './generated/LogLevel.js'
import type { LogStatus } from './generated/LogStatus.js'
import type { ManualSave } from './generated/ManualSave.js'
import type { McpTransport } from './generated/McpTransport.js'
import type { Model } from './generated/Model.js'
import type { ModelFilter } from './generated/ModelFilter.js'
import type { ModelOption } from './generated/ModelOption.js'
import type { ModelPricing } from './generated/ModelPricing.js'
import type { ModelRef } from './generated/ModelRef.js'
import type { ModelSave } from './generated/ModelSave.js'
import type { ModelType } from './generated/ModelType.js'
import type { PermissionMode } from './generated/PermissionMode.js'
import type { Preview } from './generated/Preview.js'
import type { PreviewAction } from './generated/PreviewAction.js'
import type { PreviewCurrent } from './generated/PreviewCurrent.js'
import type { PricingTier } from './generated/PricingTier.js'
import type { ProbeError } from './generated/ProbeError.js'
import type { ProbeErrorKind } from './generated/ProbeErrorKind.js'
import type { ProbeResult } from './generated/ProbeResult.js'
import type { ProbeTarget } from './generated/ProbeTarget.js'
import type { ProbedModel } from './generated/ProbedModel.js'
import type { Protocol } from './generated/Protocol.js'
import type { Provider } from './generated/Provider.js'
import type { ProviderSave } from './generated/ProviderSave.js'
import type { ReasoningConfig } from './generated/ReasoningConfig.js'
import type { ReasoningDialect } from './generated/ReasoningDialect.js'
import type { ReasoningEffort } from './generated/ReasoningEffort.js'
import type { RunAccepted } from './generated/RunAccepted.js'
import type { RunEvent } from './generated/RunEvent.js'
import type { RunRequest } from './generated/RunRequest.js'
import type { RunSnapshot } from './generated/RunSnapshot.js'
import type { RunStatus } from './generated/RunStatus.js'
import type { ScopeState } from './generated/ScopeState.js'
import type { SearchCap } from './search.js'
import type { SearchHit } from './generated/SearchHit.js'
import type { SearchKey } from './search.js'
import type { SearchPage } from './search.js'
import type { SearchPreferences } from './search.js'
import type { SearchProvider } from './search.js'
import type { SearchResult } from './search.js'
import type { SearchSave } from './search.js'
import type { SearchSettingsDto } from './search.js'
import type { ServerDto } from './generated/ServerDto.js'
import type { ServerSave } from './generated/ServerSave.js'
import type { ServerSecret } from './generated/ServerSecret.js'
import type { SkillDto } from './generated/SkillDto.js'
import type { SkillFileKind } from './generated/SkillFileKind.js'
import type { SkillFileMeta } from './generated/SkillFileMeta.js'
import type { SkillMeta } from './generated/SkillMeta.js'
import type { SkillOrigin } from './generated/SkillOrigin.js'
import type { SkillRef } from './generated/SkillRef.js'
import type { Source } from './generated/Source.js'
import type { Status } from './generated/Status.js'
import type { TokenRecord } from './generated/TokenRecord.js'
import type { TokenUsage } from './generated/TokenUsage.js'
import type { ToolDto } from './generated/ToolDto.js'
import type { ToolEffect } from './generated/ToolEffect.js'
import type { ToolOutcome } from './generated/ToolOutcome.js'
import type { ToolSourceKind } from './generated/ToolSourceKind.js'
import type { Turn } from './generated/Turn.js'
import type { TurnStatus } from './generated/TurnStatus.js'
import type { TypeSource } from './generated/TypeSource.js'
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

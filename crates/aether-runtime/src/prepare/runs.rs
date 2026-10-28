use crate::conversations::{self, ActionRequest, ReservationInput, ResolvedAction};
use crate::tools::HostTools;
use crate::{KitHost, RunConfig, RunDependencies, RunRequest};
use aether_chat::{RunInput, SystemPart};
use aether_support::{KitError, KitErrorCode};
use std::sync::Arc;

pub(crate) struct Prepared {
    pub(crate) request: RunRequest,
    pub(crate) action: ActionRequest,
    pub(crate) input: RunInput,
    pub(crate) tools: HostTools,
    #[cfg(feature = "mcp-client")]
    pub(crate) mcp_tools: bool,
    #[cfg(feature = "search")]
    pub(crate) search: Option<aether_search::execution::ResolvedConfig>,
}

fn length(field: &str, value: &str, limit: usize) -> Result<(), KitError> {
    if value.chars().count() > limit {
        return Err(KitError::bad_request(field, "too_long").with_limit(limit as u64));
    }
    Ok(())
}

pub(crate) fn check_request(request: &RunRequest, has_store: bool) -> Result<(), KitError> {
    length("input", &request.input, 100_000)?;
    length("scope", request.scope.as_deref().unwrap_or(""), 200)?;
    for (field, value) in [
        ("conversationId", &request.conversation_id),
        ("replyToId", &request.reply_to_id),
        ("retryTurnId", &request.retry_turn_id),
        ("translateTurnId", &request.translate_turn_id),
    ] {
        if let Some(value) = value {
            if value.is_empty() {
                return Err(KitError::bad_request(field, "empty"));
            }
            length(field, value, 128)?;
        }
    }
    if request.language.trim().is_empty() {
        return Err(KitError::bad_request("language", "empty"));
    }
    length("language", &request.language, 80)?;
    if request.skills.len() > 20 {
        return Err(KitError::bad_request("skills", "too_many").with_limit(20));
    }
    let retry = request.retry_turn_id.is_some() || request.regenerate;
    let translate = request.translate_turn_id.is_some();
    if retry && translate || request.regenerate && request.retry_turn_id.is_some() {
        return Err(KitError::bad_request("action", "multiple"));
    }
    if request.regenerate && !(has_store && request.conversation_id.is_some()) {
        return Err(KitError::bad_request("regenerate", "no_conversation"));
    }
    if (retry || translate || request.reply_to_id.is_some())
        && !(has_store && request.conversation_id.is_some())
    {
        return Err(KitError::bad_request("conversationId", "required"));
    }
    if request.conversation_id.is_some() && !has_store {
        return Err(KitError::unsupported("history"));
    }
    if !request.images.is_empty() && (retry || translate) {
        return Err(KitError::bad_request("images", "action_conflict"));
    }
    aether_images::check_images(&request.images)?;
    if (request.retry_turn_id.is_some() || translate)
        && (request.regenerate || request.web_search == Some(true))
    {
        return Err(KitError::bad_request("action", "conflict"));
    }
    if request.reply_to_id.is_some() && (retry || translate) {
        return Err(KitError::bad_request("replyToId", "action_conflict"));
    }
    if request.target_language.is_some() && !translate {
        return Err(KitError::bad_request(
            "targetLanguage",
            "without_translation",
        ));
    }
    if translate {
        let language = request.target_language.as_deref().unwrap_or("").trim();
        if language.is_empty() || language.chars().count() > 80 {
            return Err(KitError::bad_request("targetLanguage", "invalid"));
        }
    }
    if !retry && !translate && request.input.trim().is_empty() {
        return Err(KitError::bad_request("input", "empty"));
    }
    for (field, enabled) in [
        ("webSearch", request.web_search),
        ("generateImage", request.generate_image),
    ] {
        if enabled == Some(true) && request.purpose != "chat" {
            return Err(KitError::bad_request(field, "chat_only"));
        }
    }
    Ok(())
}

pub(crate) fn prepare(
    host: Arc<dyn KitHost>,
    config: RunConfig,
    deps: &RunDependencies,
    policy: aether_support::net::AddressPolicy,
    request: RunRequest,
    run_id: String,
) -> Result<Prepared, KitError> {
    check_request(&request, deps.conversations.is_some())?;
    let conversation = request
        .conversation_id
        .as_deref()
        .map(|id| {
            deps.conversations
                .as_ref()
                .ok_or_else(|| KitError::unsupported("history"))?
                .load(id)
        })
        .transpose()?;
    let (action, source) = conversations::resolve_action(
        conversation.as_ref(),
        request.conversation_id.clone(),
        request.scope.clone(),
        request.input.clone(),
        request.images.clone(),
        request.regenerate,
        request.reply_to_id.clone(),
        request.retry_turn_id.clone(),
        request.translate_turn_id.clone(),
        request.target_language.clone(),
    )?;
    let mut effective = request;
    effective.input.clone_from(&source.input);
    effective.images.clone_from(&source.images);
    let sourced = !matches!(action.action, ResolvedAction::Reply { .. });
    if sourced && effective.model.is_none() {
        effective.model = Some(
            source
                .source_model
                .as_ref()
                .map(|model| crate::ModelRef {
                    endpoint_id: model.endpoint_id.clone(),
                    model_id: model.model_id.clone(),
                })
                .ok_or_else(|| KitError::needs_config("source_model_unavailable"))?,
        );
    }
    let permission_mode = effective.permission_mode.unwrap_or_default();
    if !host.permission_modes(&effective).contains(&permission_mode) {
        return Err(KitError::bad_request("permission_mode", "not_allowed"));
    }
    let selected = host.select_model(&effective)?;
    let model = if sourced {
        effective.model.as_ref().unwrap_or(&selected)
    } else {
        &selected
    };
    let connection = deps.providers.resolve(model).map_err(|error| {
        if sourced && error.code() == KitErrorCode::NotFound {
            KitError::needs_config("source_model_unavailable")
        } else {
            error
        }
    })?;
    let budget = config.budget.for_connection(&connection);
    let mut system = Vec::new();
    if let Some(prompt) = host.system_prompt(&effective) {
        system.push(SystemPart::Host(prompt));
    }
    if !effective.skills.is_empty() {
        #[cfg(not(feature = "skills"))]
        return Err(KitError::unsupported("skills"));
        #[cfg(feature = "skills")]
        {
            let store = deps
                .skills
                .as_ref()
                .ok_or_else(|| KitError::bad_request("skills", "missing_instance"))?;
            system.extend(store.snapshot(&effective.skills)?.into_iter().map(|skill| {
                SystemPart::Skill {
                    id: skill.id,
                    revision: skill.revision,
                    name: skill.name,
                    body: skill.body,
                }
            }));
        }
    }
    if let Some(context) = host.context(&effective)? {
        system.push(SystemPart::Context {
            source: context.source,
            text: context.text,
        });
    }
    let specs = host
        .tools(&effective, &run_id)
        .into_iter()
        .map(|tool| tool.spec)
        .collect();
    let mut input = RunInput {
        run_id,
        purpose: effective.purpose.clone(),
        permission_mode,
        connection,
        system,
        history: Vec::new(),
        input: source.input.clone(),
        images: source.images.clone(),
        tools: specs,
        budget,
        reasoning_effort: effective.reasoning_effort.unwrap_or_default(),
        native_web_search: false,
    };
    let mut tools = HostTools::new(host);
    if effective.generate_image == Some(true) {
        if input.connection.supports_tools == Some(false) {
            return Err(KitError::bad_request("generateImage", "tools_unsupported"));
        }
        let model = tools
            .host
            .painting_model()
            .ok_or_else(|| KitError::needs_config("painting_model"))?;
        let connection = deps.providers.resolve(&model)?;
        let choices = crate::tools::painting::image_choices(&input.images, &source.history_path);
        let painting = crate::tools::painting::Painting::new(
            connection,
            policy,
            None,
            choices.into(),
            deps.conversations
                .clone()
                .zip(effective.conversation_id.clone()),
        );
        input.tools.push(painting.spec());
        tools.painting = Some(painting);
    }
    #[cfg(feature = "search")]
    let search = if effective.web_search == Some(true) {
        let store = deps
            .search
            .as_ref()
            .ok_or_else(|| KitError::bad_request("search", "missing_instance"))?;
        let preferences = store.preferences()?;
        input.native_web_search =
            preferences.model_tools_preferred && input.connection.native_web_search;
        if input.native_web_search {
            None
        } else {
            Some(store.resolve(aether_search::SearchCap::Search, None)?)
        }
    } else {
        None
    };
    #[cfg(not(feature = "search"))]
    if effective.web_search == Some(true) {
        return Err(KitError::unsupported("search"));
    }
    apply_history(&mut input, &source);
    #[cfg(feature = "mcp-client")]
    let mcp_tools = tools.host.mcp_tools(&effective);
    Ok(Prepared {
        request: effective,
        action,
        input,
        tools,
        #[cfg(feature = "search")]
        search,
        #[cfg(feature = "mcp-client")]
        mcp_tools,
    })
}

pub(crate) fn apply_history(input: &mut RunInput, source: &ReservationInput) {
    input.input.clone_from(&source.input);
    let used = input.input.chars().count()
        + input
            .system
            .iter()
            .map(|part| match part {
                SystemPart::Host(text) | SystemPart::Context { text, .. } => text.chars().count(),
                SystemPart::Skill { body, .. } => body.chars().count(),
            })
            .sum::<usize>();
    let room = input
        .budget
        .context_window
        .saturating_sub(input.budget.max_output)
        .saturating_sub(u32::try_from(used.div_ceil(3)).unwrap_or(u32::MAX));
    input.history = if input.purpose == "writing" {
        Vec::new()
    } else {
        super::trim_history(&source.history_path, room)
    };
    if !input.connection.can_vision {
        for message in &mut input.history {
            if let aether_chat::Message::UserImages { text, .. } = message {
                *message = aether_chat::Message::User(std::mem::take(text));
            }
        }
    }
}

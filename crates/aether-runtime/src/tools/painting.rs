//! Per-run painting catalogue, fixed references and result persistence.

use crate::conversations::{ConversationService, ConversationStore, Turn};
use aether_chat::{Approval, ToolCall, ToolEffect, ToolExecution, ToolSource, ToolSpec};
use aether_images::{ImagePart, ImageRequest, ImageSize, MAX_EDIT_IMAGES};
use aether_support::{Connection, KitError, KitErrorCode, net::AddressPolicy};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use tokio_util::sync::CancellationToken;

pub(crate) const GENERATE_IMAGE: &str = "generate_image";
pub(crate) const MAX_IMAGE_CHOICES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageOrigin {
    Attached,
    EarlierInput,
    Generated,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ImageChoice {
    Input(ImagePart),
    Stored {
        turn_id: String,
        generated: bool,
        idx: usize,
    },
}

impl ImageChoice {
    fn origin(&self) -> ImageOrigin {
        match self {
            Self::Input(_) => ImageOrigin::Attached,
            Self::Stored {
                generated: true, ..
            } => ImageOrigin::Generated,
            Self::Stored { .. } => ImageOrigin::EarlierInput,
        }
    }
}

pub(crate) fn image_choices(input: &[ImagePart], path: &[Turn]) -> Vec<ImageChoice> {
    let stored = path.iter().rev().flat_map(|turn| {
        let list = |generated| {
            (if generated {
                &turn.generated
            } else {
                &turn.images
            })
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, image)| image.data.is_some())
            .map(move |(idx, _)| ImageChoice::Stored {
                turn_id: turn.id.clone(),
                generated,
                idx,
            })
        };
        list(true).chain(list(false))
    });
    input
        .iter()
        .rev()
        .cloned()
        .map(ImageChoice::Input)
        .chain(stored)
        .take(MAX_IMAGE_CHOICES)
        .collect()
}

pub(crate) struct Painting {
    pub(crate) connection: Connection,
    policy: AddressPolicy,
    store: Option<(Arc<dyn ConversationStore>, String)>,
    calls: AtomicU8,
    pub(crate) choices: Arc<[ImageChoice]>,
    history: Option<(Arc<dyn ConversationStore>, String)>,
}

impl Painting {
    pub(crate) fn new(
        connection: Connection,
        policy: AddressPolicy,
        store: Option<(Arc<dyn ConversationStore>, String)>,
        choices: Arc<[ImageChoice]>,
        history: Option<(Arc<dyn ConversationStore>, String)>,
    ) -> Self {
        Self {
            connection,
            policy,
            store,
            choices,
            history,
            calls: AtomicU8::new(0),
        }
    }

    pub(crate) fn set_turn(&mut self, store: Arc<dyn ConversationStore>, turn_id: String) {
        self.store = Some((store, turn_id));
    }

    pub(crate) fn spec(&self) -> ToolSpec {
        let origins = if self.connection.can_vision {
            self.choices
                .iter()
                .map(ImageChoice::origin)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        generate_image_spec(&origins)
    }

    pub(crate) async fn execute(
        &self,
        call: &ToolCall,
        cancel: &CancellationToken,
    ) -> ToolExecution {
        if self
            .calls
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |calls| {
                (calls < 2).then_some(calls + 1)
            })
            .is_err()
        {
            return ToolExecution::Error {
                code: "limit".into(),
                message: "At most 2 images per reply.".into(),
                payload: None,
            };
        }
        let (choices, history, args) = (
            Arc::clone(&self.choices),
            self.history.clone(),
            call.args.clone(),
        );
        let can_edit = self.connection.can_vision;
        let prepared = tokio::task::spawn_blocking(move || {
            let prompt = args["prompt"]
                .as_str()
                .map(str::trim)
                .filter(|p| !p.is_empty() && p.chars().count() <= 4000)
                .ok_or_else(|| KitError::bad_request("prompt", "invalid"))?
                .to_owned();
            let size = match args.get("size") {
                None | Some(Value::Null) => ImageSize::Square,
                Some(Value::String(size)) if size == "1024x1024" => ImageSize::Square,
                Some(Value::String(size)) if size == "1024x1536" => ImageSize::Portrait,
                Some(Value::String(size)) if size == "1536x1024" => ImageSize::Landscape,
                Some(_) => return Err(KitError::bad_request("size", "invalid")),
            };
            Ok(ImageRequest {
                prompt,
                size,
                images: resolve_images(&choices, history.as_ref(), can_edit, &args)?,
            })
        })
        .await;
        let request = match prepared {
            Ok(Ok(request)) => request,
            Ok(Err(error)) => return execution_error(&error),
            Err(error) => return execution_error(&KitError::internal(error)),
        };
        if cancel.is_cancelled() {
            return execution_error(&KitError::cancelled());
        }
        let image =
            match aether_images::generate(&self.connection, &self.policy, &request, cancel).await {
                Ok(image) => image,
                Err(error)
                    if matches!(
                        error.code(),
                        KitErrorCode::Cancelled | KitErrorCode::Timeout | KitErrorCode::Network
                    ) =>
                {
                    return ToolExecution::Unknown {
                        reason: format!(
                            "Image generation was dispatched; its outcome is unknown: {}",
                            error.message()
                        ),
                    };
                }
                Err(error) => return execution_error(&error),
            };
        let payload = json!({"images": [{"mime": image.mime, "url": format!("data:{};base64,{}", image.mime, image.data)}]});
        if let Some((store, turn_id)) = &self.store {
            let (store, turn_id) = (Arc::clone(store), turn_id.clone());
            let saved = tokio::task::spawn_blocking(move || {
                ConversationService::new(store).add_generated(&turn_id, &[image])
            })
            .await;
            let failure = match saved {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.message().to_owned()),
                Err(error) => Some(error.to_string()),
            };
            if let Some(failure) = failure {
                return ToolExecution::Error {
                    code: "image_save_failed".into(),
                    message: format!(
                        "Image generation occurred, but saving failed: {failure}. Do not automatically generate again."
                    ),
                    payload: Some(payload),
                };
            }
        }
        ToolExecution::Ok {
            text: "Generated 1 image(s).".into(),
            payload: Some(payload),
        }
    }
}

fn execution_error(error: &KitError) -> ToolExecution {
    ToolExecution::Error {
        code: format!("{:?}", error.code()).to_lowercase(),
        message: error.message().to_owned(),
        payload: None,
    }
}

pub(crate) fn resolve_images(
    choices: &[ImageChoice],
    history: Option<&(Arc<dyn ConversationStore>, String)>,
    can_edit: bool,
    args: &Value,
) -> Result<Vec<ImagePart>, KitError> {
    let Some(ids) = args.get("image_ids").filter(|ids| !ids.is_null()) else {
        return Ok(Vec::new());
    };
    let ids = ids
        .as_array()
        .ok_or_else(|| KitError::bad_request("image_ids", "unknown"))?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    if !can_edit {
        return Err(KitError::bad_request("image_ids", "unsupported"));
    }
    if ids.len() > MAX_EDIT_IMAGES {
        return Err(KitError::bad_request("image_ids", "too_many"));
    }
    let picked = ids
        .iter()
        .map(|id| {
            id.as_str()
                .and_then(|id| id.strip_prefix("image-"))
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| choices.get(i))
                .ok_or_else(|| KitError::bad_request("image_ids", "unknown"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut conversation = None;
    let mut parts = Vec::with_capacity(picked.len());
    for choice in picked {
        let part = match choice {
            ImageChoice::Input(part) => Some(part.clone()),
            ImageChoice::Stored {
                turn_id,
                generated,
                idx,
            } => {
                let Some((store, id)) = history else {
                    return Err(KitError::bad_request("image_ids", "expired"));
                };
                if conversation.is_none() {
                    conversation = Some(ConversationService::new(Arc::clone(store)).get(id)?);
                }
                conversation
                    .as_ref()
                    .and_then(|c| c.turns.iter().find(|turn| &turn.id == turn_id))
                    .and_then(|turn| {
                        (if *generated {
                            &turn.generated
                        } else {
                            &turn.images
                        })
                        .get(*idx)
                    })
                    .and_then(|image| {
                        Some(ImagePart {
                            mime: image.mime.clone(),
                            data: image.data.clone()?,
                        })
                    })
            }
        };
        parts.push(part.ok_or_else(|| KitError::bad_request("image_ids", "expired"))?);
    }
    Ok(parts)
}

fn generate_image_spec(choices: &[ImageOrigin]) -> ToolSpec {
    let mut description = "Generate one image from a detailed English or Chinese prompt. Use only when the user asks for a picture, drawing, illustration or photo.".to_owned();
    let mut properties = json!({"prompt": {"type":"string","minLength":1,"maxLength":4000,"description":"What to draw."}, "size":{"type":"string","enum":["1024x1024","1024x1536","1536x1024"]}});
    if !choices.is_empty() {
        description.push_str(" To edit or use existing images as reference, pass their ids in image_ids; omit it to draw a new image.\nImages you can pass in image_ids (newest first):");
        let ids = (1..=choices.len())
            .map(|i| format!("image-{i}"))
            .collect::<Vec<_>>();
        for (id, origin) in ids.iter().zip(choices) {
            let what = match origin {
                ImageOrigin::Attached => "attached by the user in this message",
                ImageOrigin::EarlierInput => "attached in an earlier message",
                ImageOrigin::Generated => "generated in an earlier reply",
            };
            description.push_str(&format!("\n{id}: {what}"));
        }
        properties["image_ids"] = json!({"type":"array","items":{"type":"string","enum":ids},"minItems":1,"maxItems":MAX_EDIT_IMAGES,"description":"Existing images to edit or use as reference."});
    }
    ToolSpec {
        name: GENERATE_IMAGE.into(),
        description,
        parameters: json!({"type":"object","properties":properties,"required":["prompt"],"additionalProperties":false}),
        approval: Approval::Auto,
        effect: ToolEffect::Other,
        source: ToolSource::Host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spec_offers_image_ids_only_with_choices() {
        let plain = generate_image_spec(&[]);
        assert!(plain.parameters["properties"].get("image_ids").is_none());
        assert!(!plain.description.contains("image_ids"));
        let spec = generate_image_spec(&[ImageOrigin::Attached, ImageOrigin::Generated]);
        let ids = &spec.parameters["properties"]["image_ids"];
        assert_eq!(ids["items"]["enum"], json!(["image-1", "image-2"]));
        assert_eq!(
            (ids["minItems"].as_u64(), ids["maxItems"].as_u64()),
            (Some(1), Some(4))
        );
        assert!(spec.description.contains(
            "image-1: attached by the user in this message\nimage-2: generated in an earlier reply"
        ));
    }
}

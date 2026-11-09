//! Seed admitted conversation names before command dispatch can start auto-titling.
use anyhow::Result;
use zuse_engine::EngineCore;

/// Fresh launch adoption only. Retained resume does not call this helper.
pub fn seed_adoption_titles(
    core: &EngineCore,
    parent: &str,
    initial_session: &str,
    title: &str,
) -> Result<()> {
    for id in std::iter::once(parent).chain((initial_session != parent).then_some(initial_session))
    {
        if core.workspace.chat(id)?.is_some_and(|chat| {
            chat.title
                .as_deref()
                .is_none_or(|title| title.trim().is_empty())
        }) {
            core.workspace.rename_chat(id, title)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

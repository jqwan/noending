//! Unified Context Mutation Policy.
//!
//! Every context mutation — from an explicit update, the Assistant, or a direct
//! user action — passes through this policy before it touches a ContextItem; no
//! code path decides on its own whether overwriting is allowed.
//!
//! `authority` (whose word the information is: user_explicit / user_edit /
//! system_observed / agent_statement / agent_inferred) and `created_by` (who
//! performed the write) are deliberately separate: a Decision extracted from a
//! user message keeps `user_explicit` authority even though the extractor wrote
//! the row.
//!
//! Core rule: agent-derived mutations may never *silently* overwrite
//! user-explicit or user-edited context — the disagreement is persisted as a
//! ContextConflict and the user's content stays untouched.

/// Who is attempting the mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    User,
    Agent,
}

/// What kind of mutation is attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Update,
    Supersede,
    Resolve,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationDecision {
    /// Apply normally (with full revision/audit trail).
    Allow,
    /// Persist the disagreement as a ContextConflict; the existing item is
    /// left untouched. This is how background sync "asks" the user.
    CreateConflict,
    /// Interactive flows only: surface to the user before applying.
    RequireUserConfirmation,
    /// Refuse outright (e.g. unknown authority values).
    Reject,
}

pub struct AuthorityPolicy;

impl AuthorityPolicy {
    pub fn decide(existing_authority: &str, actor: Actor, op: Op) -> MutationDecision {
        match actor {
            // The user has final authority over their own context: user
            // actions may update / supersede / resolve anything.
            Actor::User => MutationDecision::Allow,
            Actor::Agent => {
                if !is_known_authority(existing_authority) {
                    return MutationDecision::Reject;
                }
                match op {
                    // Adds never overwrite anything; dedup/conflict logic
                    // still applies in the merge engine.
                    Op::Add => MutationDecision::Allow,
                    Op::Update | Op::Supersede | Op::Resolve | Op::Delete => {
                        match existing_authority {
                            "user_explicit" | "user_edit" => MutationDecision::CreateConflict,
                            // Agents may evolve their own statements and
                            // inferences, and refine system observations.
                            "agent_statement" | "agent_inferred" | "system_observed" => {
                                MutationDecision::Allow
                            }
                            _ => MutationDecision::Reject,
                        }
                    }
                }
            }
        }
    }
}

fn is_known_authority(a: &str) -> bool {
    matches!(
        a,
        "user_explicit" | "user_edit" | "system_observed" | "agent_statement" | "agent_inferred"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_never_silently_overrides_user_authority() {
        for authority in ["user_edit", "user_explicit"] {
            for op in [Op::Update, Op::Supersede, Op::Resolve, Op::Delete] {
                assert_eq!(
                    AuthorityPolicy::decide(authority, Actor::Agent, op),
                    MutationDecision::CreateConflict,
                    "agent {:?} on {} must create a conflict",
                    op,
                    authority
                );
            }
        }
    }

    #[test]
    fn agent_may_evolve_agent_and_system_authority() {
        for authority in ["agent_inferred", "agent_statement", "system_observed"] {
            assert_eq!(
                AuthorityPolicy::decide(authority, Actor::Agent, Op::Update),
                MutationDecision::Allow
            );
            assert_eq!(
                AuthorityPolicy::decide(authority, Actor::Agent, Op::Supersede),
                MutationDecision::Allow
            );
            assert_eq!(
                AuthorityPolicy::decide(authority, Actor::Agent, Op::Resolve),
                MutationDecision::Allow
            );
        }
    }

    #[test]
    fn user_actions_are_always_allowed() {
        for authority in [
            "user_explicit",
            "user_edit",
            "system_observed",
            "agent_statement",
            "agent_inferred",
        ] {
            for op in [Op::Add, Op::Update, Op::Supersede, Op::Resolve, Op::Delete] {
                assert_eq!(
                    AuthorityPolicy::decide(authority, Actor::User, op),
                    MutationDecision::Allow
                );
            }
        }
    }

    #[test]
    fn agent_add_is_never_blocked_and_unknown_authority_is_rejected() {
        assert_eq!(
            AuthorityPolicy::decide("user_explicit", Actor::Agent, Op::Add),
            MutationDecision::Allow
        );
        assert_eq!(
            AuthorityPolicy::decide("bogus", Actor::Agent, Op::Update),
            MutationDecision::Reject
        );
    }
}

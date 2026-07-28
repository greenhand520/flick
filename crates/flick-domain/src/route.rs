//! Route items returned by the Runtime router.

use crate::id::ActionId;
use serde::{Deserialize, Serialize};

/// A candidate action route returned by the router for a search query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteItem {
    pub action_id: ActionId,
    pub title: String,
    /// Score for ordering (higher = better match).
    pub score: f64,
}

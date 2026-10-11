//! 任务生命周期状态机。
//!
//! 状态定义见 [`crate::types::TaskStatus`]（与前端 JSON 契约共用同一枚举），
//! 本模块负责转移规则的判定：
//!
//! ```text
//! Idle → Parsing → Indexing → Retrieving → Planning → Generating
//!      → ApplyingPatch → Verifying → WaitingConfirm → Applied → Done
//!                          ↕ Repairing（验证失败时进入修复循环）
//! 无文件变更的完成任务：Verifying → Done（无需应用补丁）
//! 任意非终态 → Failed | Blocked | Cancelled | RolledBack
//! 终态：Done、Failed、Blocked、RolledBack、Cancelled（无出边）
//! ```

pub use crate::types::TaskStatus;

impl TaskStatus {
    /// 判断能否从当前状态转移到 `next`。
    ///
    /// 规则：
    /// - 终态不接受任何转移；
    /// - 任意非终态可以直接进入 Failed、Blocked、Cancelled 或 RolledBack；
    /// - 其余仅允许主流程中的相邻状态（见模块文档）。
    pub fn can_transition_to(&self, next: &TaskStatus) -> bool {
        if self.is_terminal() {
            return false;
        }
        match next {
            TaskStatus::Failed
            | TaskStatus::Blocked
            | TaskStatus::Cancelled
            | TaskStatus::RolledBack => true,
            _ => self.next_in_flow().contains(next),
        }
    }

    /// 是否为终态。终态是生命周期的终点，之后不再发生任何转移。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Done
                | TaskStatus::Failed
                | TaskStatus::Blocked
                | TaskStatus::RolledBack
                | TaskStatus::Cancelled
        )
    }

    /// 主流程允许的直接后继状态；通配的 Failed/Cancelled/RolledBack 不在此列，
    /// 由 [`TaskStatus::can_transition_to`] 统一处理。
    fn next_in_flow(&self) -> &'static [TaskStatus] {
        match self {
            TaskStatus::Idle => &[TaskStatus::Parsing],
            TaskStatus::Parsing => &[TaskStatus::Indexing],
            TaskStatus::Indexing => &[TaskStatus::Retrieving],
            TaskStatus::Retrieving => &[TaskStatus::Planning],
            TaskStatus::Planning => &[TaskStatus::Generating],
            TaskStatus::Generating => &[TaskStatus::ApplyingPatch],
            TaskStatus::ApplyingPatch => &[TaskStatus::Verifying],
            TaskStatus::Verifying => &[
                TaskStatus::Repairing,
                TaskStatus::WaitingConfirm,
                TaskStatus::Done,
            ],
            TaskStatus::Repairing => &[TaskStatus::Verifying, TaskStatus::WaitingConfirm],
            TaskStatus::WaitingConfirm => &[TaskStatus::Applied],
            TaskStatus::Applied => &[TaskStatus::Done],
            TaskStatus::Done
            | TaskStatus::Failed
            | TaskStatus::Blocked
            | TaskStatus::RolledBack
            | TaskStatus::Cancelled => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TaskStatus as S;

    #[test]
    fn repair_lifecycle_statuses_roundtrip_through_store() {
        use crate::store::{project, task, Store};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let store = Store::new(&path).unwrap();
        let id = {
            let conn = store.conn();
            let project_id = project::insert_project(&conn, "project", "project").unwrap();
            task::insert_task(&conn, project_id, "repair").unwrap()
        };
        let flow = [
            S::Parsing,
            S::Indexing,
            S::Retrieving,
            S::Planning,
            S::Generating,
            S::ApplyingPatch,
            S::Verifying,
            S::Repairing,
            S::Verifying,
            S::WaitingConfirm,
            S::Applied,
            S::Done,
        ];
        for next in flow {
            let conn = store.conn();
            let current = task::get_task(&conn, id).unwrap().unwrap().status;
            assert!(current.can_transition_to(&next), "{current:?} -> {next:?}");
            if next.is_terminal() {
                task::finish_task(&conn, id, next).unwrap();
            } else {
                task::update_task_status(&conn, id, next).unwrap();
            }
            assert_eq!(task::get_task(&conn, id).unwrap().unwrap().status, next);
        }
        drop(store);
        let reopened = Store::new(&path).unwrap();
        let task = task::get_task(&reopened.conn(), id).unwrap().unwrap();
        assert_eq!(task.status, S::Done);
        assert!(task.status.is_terminal());
        assert!(task.finished_at.is_some());
        assert!(task.duration_ms.unwrap() >= 0);
    }

    /// 全部 16 个状态，顺序与规范一致。
    const ALL: [S; 16] = [
        S::Idle,
        S::Parsing,
        S::Indexing,
        S::Retrieving,
        S::Planning,
        S::Generating,
        S::ApplyingPatch,
        S::Verifying,
        S::Repairing,
        S::WaitingConfirm,
        S::Applied,
        S::Done,
        S::Failed,
        S::Blocked,
        S::RolledBack,
        S::Cancelled,
    ];

    const NON_TERMINAL: [S; 11] = [
        S::Idle,
        S::Parsing,
        S::Indexing,
        S::Retrieving,
        S::Planning,
        S::Generating,
        S::ApplyingPatch,
        S::Verifying,
        S::Repairing,
        S::WaitingConfirm,
        S::Applied,
    ];

    const TERMINAL: [S; 5] = [S::Done, S::Failed, S::Blocked, S::RolledBack, S::Cancelled];

    /// 规范中的主流程转移表（不含通配的 Failed/Cancelled/RolledBack）。
    fn in_flow_allows(from: S, to: S) -> bool {
        matches!(
            (from, to),
            (S::Idle, S::Parsing)
                | (S::Parsing, S::Indexing)
                | (S::Indexing, S::Retrieving)
                | (S::Retrieving, S::Planning)
                | (S::Planning, S::Generating)
                | (S::Generating, S::ApplyingPatch)
                | (S::ApplyingPatch, S::Verifying)
                | (S::Verifying, S::Repairing)
                | (S::Verifying, S::WaitingConfirm)
                | (S::Verifying, S::Done)
                | (S::Repairing, S::Verifying)
                | (S::Repairing, S::WaitingConfirm)
                | (S::WaitingConfirm, S::Applied)
                | (S::WaitingConfirm, S::Failed)
                | (S::Applied, S::Done)
        )
    }

    /// 规范定义的完整合法转移集合，与实现相互独立，作为测试基准。
    fn spec_allows(from: S, to: S) -> bool {
        in_flow_allows(from, to)
            || (!TERMINAL.contains(&from)
                && matches!(to, S::Failed | S::Blocked | S::Cancelled | S::RolledBack))
    }

    #[test]
    fn happy_path_is_allowed() {
        let path = [
            S::Idle,
            S::Parsing,
            S::Indexing,
            S::Retrieving,
            S::Planning,
            S::Generating,
            S::ApplyingPatch,
            S::Verifying,
            S::WaitingConfirm,
            S::Applied,
            S::Done,
        ];
        for pair in path.windows(2) {
            assert!(
                pair[0].can_transition_to(&pair[1]),
                "{:?} -> {:?} 应当允许",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn repair_loop_is_allowed() {
        assert!(S::Verifying.can_transition_to(&S::Repairing));
        assert!(S::Repairing.can_transition_to(&S::Verifying));
        assert!(S::Repairing.can_transition_to(&S::WaitingConfirm));
    }

    #[test]
    fn any_non_terminal_state_can_abort() {
        for from in NON_TERMINAL {
            for to in [S::Failed, S::Cancelled, S::RolledBack] {
                assert!(
                    from.can_transition_to(&to),
                    "{:?} -> {:?} 应当允许（通配中止）",
                    from,
                    to
                );
            }
        }
    }

    /// 穷举 16×16 状态矩阵：合法集合之外的转移（跳步、回退、自环、终态出边）必须全部被拒绝。
    #[test]
    fn full_transition_matrix_matches_spec() {
        for from in ALL {
            for to in ALL {
                assert_eq!(
                    from.can_transition_to(&to),
                    spec_allows(from, to),
                    "{:?} -> {:?} 与规范不符",
                    from,
                    to
                );
            }
        }
    }

    #[test]
    fn terminal_states_reject_everything() {
        for from in TERMINAL {
            for to in ALL {
                assert!(
                    !from.can_transition_to(&to),
                    "终态 {:?} 不应再转移到 {:?}",
                    from,
                    to
                );
            }
        }
    }

    #[test]
    fn is_terminal_matches_spec() {
        for status in ALL {
            assert_eq!(
                status.is_terminal(),
                TERMINAL.contains(&status),
                "{:?} 的终态判定与规范不符",
                status
            );
        }
    }
}

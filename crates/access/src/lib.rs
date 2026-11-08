//! Workspace access policy (D030, docs/workspaces/ARCHITECTURE.md §3, §5, §8).
//! The hub enforces every read, write, search, graph and download with `decide`; the desktop uses it only to hide
//! controls. Deny by default; a project outside a member's scope is `NotFound`, never `Forbidden`.
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Owner,
    Admin,
    Editor,
    Contributor,
    Viewer,
}

/// Everything a member can be allowed to do. Deliberately has no "execute", "shell" or "launch" permission:
/// membership never grants running anything on anyone's machine (§8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Perm {
    WorkspaceRead,
    ProjectRead,
    TaskRead,
    DocRead,
    MediaRead,
    TaskCreate,
    TaskEditOwn,
    MediaUpload,
    TaskEditAny,
    TaskAssign,
    MilestoneEdit,
    DocPublish,
    ProjectLink,
    MembersInvite,
    AuditRead,
    MembersManage,
    WorkspaceManage,
    WorkspaceDelete,
    OwnershipTransfer,
}

use Perm::*;

impl Perm {
    pub const ALL: [Perm; 19] = [
        WorkspaceRead, ProjectRead, TaskRead, DocRead, MediaRead, TaskCreate, TaskEditOwn, MediaUpload, TaskEditAny,
        TaskAssign, MilestoneEdit, DocPublish, ProjectLink, MembersInvite, AuditRead, MembersManage, WorkspaceManage,
        WorkspaceDelete, OwnershipTransfer,
    ];
    /// Only an owner holds these; a custom grant can't hand them out.
    /// MembersInvite = deciding who is in the workspace at all (invite, add, remove): the owner's call alone (D037).
    pub fn owner_only(self) -> bool {
        matches!(self, WorkspaceDelete | OwnershipTransfer | MembersInvite)
    }
    /// Can come only from the role, never from a custom grant: they control who gets access.
    pub fn grantable(self) -> bool {
        !self.owner_only() && !matches!(self, MembersInvite | MembersManage | WorkspaceManage)
    }
    /// Workspace-wide administration: unavailable to a member limited to some projects, whatever their role,
    /// so a project scope can't be escaped by inviting or re-scoping members or reading the whole audit log.
    fn workspace_admin(self) -> bool {
        matches!(self, MembersInvite | MembersManage | AuditRead | WorkspaceManage | WorkspaceDelete | OwnershipTransfer)
    }
    /// Checked against the project scope (workspace-level permissions are not).
    fn project_level(self) -> bool {
        matches!(self, ProjectRead | TaskRead | DocRead | MediaRead | TaskCreate | TaskEditOwn | MediaUpload | TaskEditAny | TaskAssign | MilestoneEdit | DocPublish | ProjectLink)
    }
    /// The read permission that decides whether a refused caller may know the resource exists.
    fn read_of(self) -> Perm {
        match self {
            TaskCreate | TaskEditOwn | TaskEditAny | TaskAssign | MilestoneEdit => TaskRead,
            DocPublish => DocRead,
            MediaUpload => MediaRead,
            ProjectLink => ProjectRead,
            p if p.project_level() => p,
            _ => WorkspaceRead,
        }
    }
}

impl Perm {
    /// Stored/wire name, as in the architecture table: `TaskEditOwn` → `task.edit_own`.
    pub fn name(self) -> String {
        let mut words = Vec::new();
        for c in format!("{self:?}").chars() {
            if c.is_uppercase() {
                words.push(String::new());
            }
            words.last_mut().expect("starts uppercase").push(c.to_ascii_lowercase());
        }
        format!("{}.{}", words[0], words[1..].join("_"))
    }
    pub fn parse(s: &str) -> Option<Perm> {
        Perm::ALL.into_iter().find(|p| p.name() == s)
    }
}

impl Role {
    pub const ALL: [Role; 5] = [Role::Owner, Role::Admin, Role::Editor, Role::Contributor, Role::Viewer];
    pub fn name(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Editor => "editor",
            Role::Contributor => "contributor",
            Role::Viewer => "viewer",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.name() == s)
    }
    pub fn allows(self, p: Perm) -> bool {
        let reads = matches!(p, WorkspaceRead | ProjectRead | TaskRead | DocRead | MediaRead);
        let contribute = matches!(p, TaskCreate | TaskEditOwn | MediaUpload);
        let edit = matches!(p, TaskEditAny | TaskAssign | MilestoneEdit | DocPublish);
        match self {
            Role::Owner => true,
            Role::Admin => !p.owner_only(),
            Role::Editor => reads || contribute || edit,
            Role::Contributor => reads || contribute,
            Role::Viewer => reads,
        }
    }
}

/// One user's membership in one workspace (users hold one per workspace, each with its own role).
#[derive(Clone, Debug)]
pub struct Membership {
    pub user: String,
    pub workspace: String,
    pub role: Role,
    pub grant: BTreeSet<Perm>,
    pub deny: BTreeSet<Perm>,
    /// None = every project in the workspace; Some = only these project ids.
    pub projects: Option<BTreeSet<String>>,
    pub revoked: bool,
}

impl Membership {
    pub fn new(user: &str, workspace: &str, role: Role) -> Self {
        Membership { user: user.into(), workspace: workspace.into(), role, grant: BTreeSet::new(), deny: BTreeSet::new(), projects: None, revoked: false }
    }
    fn in_scope(&self, project: &str) -> bool {
        self.projects.as_ref().is_none_or(|s| s.contains(project))
    }
    /// Role + grants − denies, ignoring per-resource rules.
    pub fn has(&self, p: Perm) -> bool {
        !self.deny.contains(&p) && (self.role.allows(p) || (self.grant.contains(&p) && p.grantable()))
    }
}

/// What an action is aimed at.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    Workspace(&'a str),
    Project { workspace: &'a str, project: &'a str },
    /// A task, milestone, doc or media item inside a project; `created_by` drives TaskEditOwn.
    Item { workspace: &'a str, project: &'a str, created_by: &'a str },
    /// A member (MembersManage) or an invitation (MembersInvite). `role` = their current role, or the role an
    /// invitation grants; `new_role` = the role being assigned, if any. Every role involved must rank strictly below
    /// the caller's: admins handle editors and below, owners also admins; owners are made only by OwnershipTransfer.
    Member { workspace: &'a str, role: Role, new_role: Option<Role> },
}

impl Target<'_> {
    fn workspace(&self) -> &str {
        match *self {
            Target::Workspace(w) | Target::Project { workspace: w, .. } | Target::Item { workspace: w, .. } | Target::Member { workspace: w, .. } => w,
        }
    }
    fn project(&self) -> Option<&str> {
        match *self {
            Target::Project { project, .. } | Target::Item { project, .. } => Some(project),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// The caller may see the resource but not do this.
    Forbidden,
    /// Indistinguishable from a resource that doesn't exist.
    NotFound,
}

/// The single authorization check. `m` is the caller's membership in the target's workspace, if any.
pub fn decide(m: Option<&Membership>, p: Perm, t: Target) -> Decision {
    let Some(m) = m.filter(|m| !m.revoked && m.workspace == t.workspace()) else { return Decision::NotFound };
    if let Some(project) = t.project() {
        if !m.in_scope(project) {
            return Decision::NotFound;
        }
    }
    let rules_ok = match (p, t) {
        // A scoped member works inside their projects only; workspace-wide project actions need the whole workspace.
        (_, Target::Workspace(_)) if p.project_level() && m.projects.is_some() => false,
        (TaskEditOwn, Target::Item { created_by, .. }) => created_by == m.user,
        (TaskEditOwn, _) => false,
        _ if p.workspace_admin() && m.projects.is_some() => false,
        // Enum order is rank: Owner < Admin < Editor < …, so "strictly below the caller" is `r > m.role`.
        (MembersManage | MembersInvite, Target::Member { role, new_role, .. }) => role > m.role && new_role.is_none_or(|r| r > m.role),
        (MembersManage | MembersInvite, _) => false,
        _ => true,
    };
    if m.has(p) && rules_ok {
        return Decision::Allow;
    }
    if m.has(p.read_of()) { Decision::Forbidden } else { Decision::NotFound }
}

/// Editing a task: anyone with TaskEditAny, or its author with TaskEditOwn.
pub fn can_edit_item(m: Option<&Membership>, workspace: &str, project: &str, created_by: &str) -> Decision {
    let t = Target::Item { workspace, project, created_by };
    match decide(m, TaskEditAny, t) {
        Decision::Allow => Decision::Allow,
        _ => decide(m, TaskEditOwn, t),
    }
}

/* ---------- graph (§5): only readable nodes, only edges with both ends readable ---------- */

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub workspace: String,
    pub project: String,
    pub label: String,
}

/// `members` = every membership the caller holds (one per workspace). Returns kept nodes and the edges between them;
/// hidden nodes leave no trace (no counts, no dangling ids).
pub fn filter_graph(members: &[Membership], nodes: &[Node], edges: &[(usize, usize)]) -> (Vec<Node>, Vec<(usize, usize)>) {
    let mut map = vec![None; nodes.len()];
    let mut kept = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        let m = members.iter().find(|m| m.workspace == n.workspace);
        if decide(m, ProjectRead, Target::Project { workspace: &n.workspace, project: &n.project }) == Decision::Allow {
            map[i] = Some(kept.len());
            kept.push(n.clone());
        }
    }
    let e = edges.iter().filter_map(|&(a, b)| Some((map.get(a).copied().flatten()?, map.get(b).copied().flatten()?))).collect();
    (kept, e)
}

/* ---------- publication allowlist (§3): the only record kinds that may ever leave a PC ---------- */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publishable {
    Task,
    Milestone,
    Summary,
    NextAction,
    Progress,
    ActivityCount,
    ApprovedDoc,
    Media,
    /// A linked project's file list + map, sent only after the owner approves a preview (D035). Never file contents.
    Snapshot,
}

/// Unknown kinds are private. Sessions, transcripts, credentials, settings, paths, launches, discoveries,
/// phone devices and vault notes have no entry here and so can never be published.
pub fn publishable(kind: &str) -> Option<Publishable> {
    Some(match kind {
        "task" => Publishable::Task,
        "milestone" => Publishable::Milestone,
        "summary" => Publishable::Summary,
        "next_action" => Publishable::NextAction,
        "progress" => Publishable::Progress,
        "activity_count" => Publishable::ActivityCount,
        "approved_doc" => Publishable::ApprovedDoc,
        "media" => Publishable::Media,
        "snapshot" => Publishable::Snapshot,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use Decision::*;

    fn proj<'a>(w: &'a str, p: &'a str) -> Target<'a> {
        Target::Project { workspace: w, project: p }
    }
    fn item<'a>(w: &'a str, p: &'a str, by: &'a str) -> Target<'a> {
        Target::Item { workspace: w, project: p, created_by: by }
    }

    #[test]
    fn role_matrix_matches_the_architecture_table() {
        let row = |r: Role| Perm::ALL.iter().filter(|&&p| r.allows(p)).count();
        assert_eq!(row(Role::Owner), 19);
        assert_eq!(row(Role::Admin), 16);
        assert_eq!(row(Role::Editor), 12);
        assert_eq!(row(Role::Contributor), 8);
        assert_eq!(row(Role::Viewer), 5);
        assert!(Role::Viewer.allows(TaskRead) && !Role::Viewer.allows(TaskCreate));
        assert!(Role::Contributor.allows(TaskEditOwn) && !Role::Contributor.allows(TaskEditAny));
        assert!(Role::Editor.allows(TaskAssign) && !Role::Editor.allows(MembersInvite));
        assert!(Role::Admin.allows(MembersManage) && !Role::Admin.allows(WorkspaceDelete) && !Role::Admin.allows(MembersInvite));
    }

    #[test]
    fn no_membership_or_other_workspace_or_revoked_is_not_found() {
        let m = Membership::new("u", "flywing", Role::Owner);
        assert_eq!(decide(None, TaskRead, proj("flywing", "site")), NotFound);
        assert_eq!(decide(Some(&m), TaskRead, proj("oara", "game")), NotFound, "membership is per workspace");
        let mut r = m.clone();
        r.revoked = true;
        assert_eq!(decide(Some(&r), WorkspaceRead, Target::Workspace("flywing")), NotFound);
    }

    #[test]
    fn same_user_different_roles_per_workspace() {
        let fly = Membership::new("u", "flywing", Role::Editor);
        let bomb = Membership::new("u", "bombsquad", Role::Viewer);
        assert_eq!(decide(Some(&fly), TaskEditAny, item("flywing", "site", "x")), Allow);
        assert_eq!(decide(Some(&bomb), TaskEditAny, item("bombsquad", "merch", "x")), Forbidden);
    }

    #[test]
    fn project_scope_hides_projects_as_not_found() {
        let mut m = Membership::new("u", "flywing", Role::Editor);
        m.projects = Some(["media".to_string()].into());
        assert_eq!(decide(Some(&m), TaskRead, proj("flywing", "media")), Allow);
        assert_eq!(decide(Some(&m), TaskRead, proj("flywing", "contracts")), NotFound);
        assert_eq!(decide(Some(&m), TaskEditAny, item("flywing", "contracts", "x")), NotFound);
        // Workspace-wide project actions need the whole workspace (Forbidden: they already know the workspace exists).
        assert_eq!(decide(Some(&m), ProjectRead, Target::Workspace("flywing")), Forbidden);
        assert_eq!(decide(Some(&m), WorkspaceRead, Target::Workspace("flywing")), Allow);
    }

    #[test]
    fn deny_beats_grant_beats_role_and_owner_only_cant_be_granted() {
        let mut m = Membership::new("u", "w", Role::Viewer);
        m.grant.insert(TaskCreate);
        assert_eq!(decide(Some(&m), TaskCreate, proj("w", "p")), Allow);
        m.deny.insert(TaskCreate);
        assert_eq!(decide(Some(&m), TaskCreate, proj("w", "p")), Forbidden);
        let mut e = Membership::new("u", "w", Role::Editor);
        e.deny.insert(TaskRead);
        assert_eq!(decide(Some(&e), TaskRead, proj("w", "p")), NotFound, "denied read hides the item");
        let mut a = Membership::new("u", "w", Role::Admin);
        a.grant.insert(WorkspaceDelete);
        assert_eq!(decide(Some(&a), WorkspaceDelete, Target::Workspace("w")), Forbidden);
    }

    #[test]
    fn edit_own_only_for_the_author() {
        let m = Membership::new("u", "w", Role::Contributor);
        assert_eq!(can_edit_item(Some(&m), "w", "p", "u"), Allow);
        assert_eq!(can_edit_item(Some(&m), "w", "p", "someone"), Forbidden);
        let e = Membership::new("u", "w", Role::Editor);
        assert_eq!(can_edit_item(Some(&e), "w", "p", "someone"), Allow);
    }

    #[test]
    fn admins_cant_manage_owners_or_admins() {
        let a = Membership::new("u", "w", Role::Admin);
        let at = |role| Target::Member { workspace: "w", role, new_role: None };
        assert_eq!(decide(Some(&a), MembersManage, at(Role::Editor)), Allow);
        assert_eq!(decide(Some(&a), MembersManage, at(Role::Admin)), Forbidden);
        assert_eq!(decide(Some(&a), MembersManage, at(Role::Owner)), Forbidden);
        let o = Membership::new("u", "w", Role::Owner);
        assert_eq!(decide(Some(&o), MembersManage, at(Role::Admin)), Allow);
        let ed = Membership::new("u", "w", Role::Editor);
        assert_eq!(decide(Some(&ed), MembersManage, at(Role::Viewer)), Forbidden);
    }

    #[test]
    fn role_changes_and_invites_stay_below_the_caller() {
        let a = Membership::new("u", "w", Role::Admin);
        let change = |role, to| Target::Member { workspace: "w", role, new_role: Some(to) };
        assert_eq!(decide(Some(&a), MembersManage, change(Role::Viewer, Role::Editor)), Allow);
        assert_eq!(decide(Some(&a), MembersManage, change(Role::Viewer, Role::Admin)), Forbidden, "admin can't promote to admin");
        assert_eq!(decide(Some(&a), MembersManage, change(Role::Viewer, Role::Owner)), Forbidden);
        assert_eq!(decide(Some(&a), MembersInvite, Target::Member { workspace: "w", role: Role::Admin, new_role: None }), Forbidden);
        assert_eq!(decide(Some(&a), MembersInvite, Target::Member { workspace: "w", role: Role::Editor, new_role: None }), Forbidden, "only the owner decides who is in");
        assert_eq!(decide(Some(&Membership::new("o", "w", Role::Owner)), MembersInvite, Target::Member { workspace: "w", role: Role::Admin, new_role: None }), Allow);
        assert_eq!(decide(Some(&a), MembersManage, Target::Workspace("w")), Forbidden, "only with a member target");
        let o = Membership::new("u", "w", Role::Owner);
        assert_eq!(decide(Some(&o), MembersManage, change(Role::Editor, Role::Admin)), Allow);
        assert_eq!(decide(Some(&o), MembersManage, change(Role::Admin, Role::Owner)), Forbidden, "owners only via OwnershipTransfer");
        // Who-gets-access permissions can't be handed out by custom grant.
        let mut e = Membership::new("u", "w", Role::Editor);
        e.grant.extend([MembersInvite, MembersManage, WorkspaceManage]);
        assert_eq!(decide(Some(&e), MembersInvite, Target::Member { workspace: "w", role: Role::Viewer, new_role: None }), Forbidden);
    }

    #[test]
    fn a_project_scope_cant_be_escaped_through_admin_rights() {
        let mut a = Membership::new("u", "w", Role::Admin);
        a.projects = Some(["media".to_string()].into());
        let viewer = Target::Member { workspace: "w", role: Role::Viewer, new_role: None };
        assert_eq!(decide(Some(&a), MembersInvite, viewer), Forbidden);
        assert_eq!(decide(Some(&a), MembersManage, viewer), Forbidden);
        assert_eq!(decide(Some(&a), AuditRead, Target::Workspace("w")), Forbidden);
        assert_eq!(decide(Some(&a), WorkspaceManage, Target::Workspace("w")), Forbidden);
        assert_eq!(decide(Some(&a), TaskEditAny, item("w", "media", "x")), Allow, "still full rights inside the scope");
    }

    #[test]
    fn graph_shows_only_readable_nodes_and_edges_between_them() {
        let n = |w: &str, p: &str| Node { workspace: w.into(), project: p.into(), label: p.to_uppercase() };
        let nodes = vec![n("flywing", "site"), n("flywing", "contracts"), n("oara", "game"), n("bombsquad", "merch")];
        let edges = vec![(0, 1), (0, 2), (2, 3), (1, 3), (0, 3)];
        let mut fly = Membership::new("u", "flywing", Role::Viewer);
        fly.projects = Some(["site".to_string()].into());
        let bomb = Membership::new("u", "bombsquad", Role::Viewer);
        let (kept, e) = filter_graph(&[fly, bomb], &nodes, &edges);
        let labels: Vec<_> = kept.iter().map(|k| k.label.as_str()).collect();
        assert_eq!(labels, ["SITE", "MERCH"], "contracts (out of scope) and oara (no membership) are gone");
        assert_eq!(e, vec![(0, 1)], "only site↔merch survives; no edge points at a hidden node");
        assert!(filter_graph(&[], &nodes, &edges).0.is_empty());
    }

    #[test]
    fn private_kinds_can_never_be_published() {
        for k in ["session", "transcript", "chat", "credential", "token", "setting", "path", "launch", "discovery", "phone_device", "vault_note", "commit_message", "", "TASK"] {
            assert_eq!(publishable(k), None, "{k}");
        }
        assert_eq!(publishable("task"), Some(Publishable::Task));
        assert_eq!(publishable("approved_doc"), Some(Publishable::ApprovedDoc));
        assert_eq!(publishable("snapshot"), Some(Publishable::Snapshot));
    }

    #[test]
    fn names_round_trip() {
        assert_eq!(TaskEditOwn.name(), "task.edit_own");
        assert_eq!(WorkspaceRead.name(), "workspace.read");
        for p in Perm::ALL {
            assert_eq!(Perm::parse(&p.name()), Some(p));
        }
        for r in Role::ALL {
            assert_eq!(Role::parse(r.name()), Some(r));
        }
        assert_eq!(Perm::parse("TaskRead"), None);
    }

    #[test]
    fn membership_never_grants_execution() {
        for p in Perm::ALL {
            let name = format!("{p:?}").to_lowercase();
            for bad in ["exec", "shell", "launch", "run", "cli", "command"] {
                assert!(!name.contains(bad), "{name}");
            }
        }
    }
}

//! Role-based access control.
//!
//! Each module declares the [`Permission`]s it checks and which roles get them. A role also
//! has every permission of the roles below it, and root has every permission there is.
//! Whether a permission applies to a particular record is decided by its [`Scope`].

use std::collections::{HashMap, HashSet};

use kippu_domain::account::Role;
use kippu_domain::{AccountId, OrganizationId};

use super::Principal;

/// Something a principal may be allowed to do, e.g. `events.write`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Permission(&'static str);

impl Permission {
    /// Declares a permission. Use a `module.action` name.
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// The permission's name.
    pub const fn name(self) -> &'static str {
        self.0
    }
}

/// Which records an action touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Not tied to an owner, e.g. creating an organization.
    Global,
    /// Records belonging to an organization. Organizers act only within their own.
    Organization(OrganizationId),
    /// Records belonging to an account. Users act only on their own.
    Account(AccountId),
}

/// Which roles hold which permissions.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    grants: HashMap<Role, HashSet<Permission>>,
}

impl Policy {
    /// Gives `role` (and every role above it) a permission.
    pub fn grant(&mut self, role: Role, permission: Permission) {
        self.grants.entry(role).or_default().insert(permission);
    }

    /// Whether `role` holds `permission`, directly or through a lower role.
    pub fn allows(&self, role: Role, permission: Permission) -> bool {
        role == Role::Root
            || self.grants.iter().any(|(&granted_to, permissions)| {
                granted_to <= role && permissions.contains(&permission)
            })
    }

    /// Whether `principal` may perform `permission` on records in `scope`.
    pub fn permits(&self, principal: &Principal, permission: Permission, scope: Scope) -> bool {
        let role = principal.role();
        if !self.allows(role, permission) {
            return false;
        }
        match (principal, scope) {
            (Principal::Root { .. }, _) | (_, Scope::Global) => true,
            (Principal::Account { role, .. }, _) if *role >= Role::Admin => true,
            (
                Principal::Account {
                    organizations,
                    role,
                    ..
                },
                Scope::Organization(organization),
            ) => *role == Role::Organizer && organizations.contains(&organization),
            (Principal::Account { id, .. }, Scope::Account(owner)) => *id == owner,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READ: Permission = Permission::new("things.read");
    const WRITE: Permission = Permission::new("things.write");

    fn account(role: Role, organizations: Vec<OrganizationId>) -> Principal {
        Principal::Account {
            id: AccountId::generate(),
            role,
            organizations,
        }
    }

    fn policy() -> Policy {
        let mut policy = Policy::default();
        policy.grant(Role::User, READ);
        policy.grant(Role::Organizer, WRITE);
        policy
    }

    #[test]
    fn higher_roles_inherit_lower_grants() {
        let policy = policy();
        assert!(policy.allows(Role::Admin, READ));
        assert!(policy.allows(Role::Admin, WRITE));
        assert!(!policy.allows(Role::User, WRITE));
        assert!(policy.allows(Role::Root, Permission::new("anything.at.all")));
    }

    #[test]
    fn organizers_act_only_within_their_organizations() {
        let policy = policy();
        let mine = OrganizationId::generate();
        let theirs = OrganizationId::generate();
        let organizer = account(Role::Organizer, vec![mine]);
        assert!(policy.permits(&organizer, WRITE, Scope::Organization(mine)));
        assert!(!policy.permits(&organizer, WRITE, Scope::Organization(theirs)));
        assert!(policy.permits(
            &account(Role::Admin, vec![]),
            WRITE,
            Scope::Organization(theirs)
        ));
    }

    #[test]
    fn users_act_only_on_their_own_records() {
        let policy = policy();
        let user = account(Role::User, vec![]);
        let Principal::Account { id, .. } = user else {
            unreachable!()
        };
        assert!(policy.permits(&user, READ, Scope::Account(id)));
        assert!(!policy.permits(&user, READ, Scope::Account(AccountId::generate())));
        assert!(!policy.permits(&user, WRITE, Scope::Account(id)));
    }
}

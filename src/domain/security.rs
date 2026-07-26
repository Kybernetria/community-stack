#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrincipalRole {
    App,
    Admin,
    Transport,
}

#[derive(Clone, Debug)]
pub struct Principal {
    pub id: String,
    pub role: PrincipalRole,
}

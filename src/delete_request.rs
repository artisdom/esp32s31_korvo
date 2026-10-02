//! Confirmation pins the exact filename and never authorizes a busy delete.
#[derive(Default)]
pub struct DeleteRequest<T> {
    name: Option<T>,
}
impl<T> DeleteRequest<T> {
    pub fn new() -> Self {
        Self { name: None }
    }
    pub fn arm(&mut self, name: T, busy: bool) {
        self.name = if busy { None } else { Some(name) };
    }
    pub fn name(&self) -> Option<&T> {
        self.name.as_ref()
    }
    pub fn cancel(&mut self) {
        self.name = None;
    }
    pub fn confirm(&mut self, busy: bool) -> Option<T> {
        let name = self.name.take();
        if busy { None } else { name }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_is_bound_to_the_requested_filename() {
        let mut r = DeleteRequest::new();
        r.arm("REC00001.WAV", false);
        assert_eq!(r.name(), Some(&"REC00001.WAV"));
        assert_eq!(r.confirm(false), Some("REC00001.WAV"));
        assert_eq!(r.confirm(false), None);
    }
    #[test]
    fn cancel_and_busy_state_never_authorize_deletion() {
        let mut r = DeleteRequest::new();
        r.arm("A.WAV", true);
        assert_eq!(r.confirm(false), None);
        r.arm("A.WAV", false);
        assert_eq!(r.confirm(true), None);
        assert_eq!(r.confirm(false), None);
        r.arm("A.WAV", false);
        r.cancel();
        assert_eq!(r.confirm(false), None);
    }
}

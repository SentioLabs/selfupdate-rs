use crate::{Channel, Error, Result};

/// Adapt the consumer's persisted configuration to update channels.
pub trait Store {
    /// Read the stored channel; empty values are treated as stable by `Updater`.
    fn channel(&self) -> Result<Channel>;
    /// Persist a selected channel.
    fn set_channel(&mut self, channel: Channel) -> Result<()>;
}

impl<T: Store + ?Sized> Store for &mut T {
    fn channel(&self) -> Result<Channel> {
        (**self).channel()
    }
    fn set_channel(&mut self, channel: Channel) -> Result<()> {
        (**self).set_channel(channel)
    }
}

impl<T: Store + ?Sized> Store for Box<T> {
    fn channel(&self) -> Result<Channel> {
        (**self).channel()
    }
    fn set_channel(&mut self, channel: Channel) -> Result<()> {
        (**self).set_channel(channel)
    }
}

/// In-memory storage. An empty channel reads as stable.
#[derive(Debug, Default)]
pub struct MemStore {
    /// Current in-memory value.
    pub current: Channel,
}
impl Store for MemStore {
    fn channel(&self) -> Result<Channel> {
        Ok(if self.current.as_str().is_empty() {
            Channel::STABLE
        } else {
            self.current.clone()
        })
    }
    fn set_channel(&mut self, channel: Channel) -> Result<()> {
        self.current = channel;
        Ok(())
    }
}

/// A closure-backed store. Missing closures return `Error::NoStore`.
#[derive(Default)]
pub struct FuncStore<'a> {
    /// Read configuration.
    pub get: Option<Box<dyn Fn() -> Result<Channel> + 'a>>,
    /// Update and save configuration.
    pub set: Option<Box<dyn FnMut(Channel) -> Result<()> + 'a>>,
}
impl<'a> FuncStore<'a> {
    /// Adapt a pair of configuration closures.
    pub fn new(
        get: impl Fn() -> Result<Channel> + 'a,
        set: impl FnMut(Channel) -> Result<()> + 'a,
    ) -> Self {
        Self {
            get: Some(Box::new(get)),
            set: Some(Box::new(set)),
        }
    }
}
impl Store for FuncStore<'_> {
    fn channel(&self) -> Result<Channel> {
        self.get.as_ref().ok_or(Error::NoStore)?()
    }
    fn set_channel(&mut self, channel: Channel) -> Result<()> {
        self.set.as_mut().ok_or(Error::NoStore)?(channel)
    }
}

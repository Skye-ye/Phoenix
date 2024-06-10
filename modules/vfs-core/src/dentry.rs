use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::{Arc, Weak},
};
use core::{mem::MaybeUninit, str::FromStr};

use sync::mutex::spin_mutex::SpinMutex;
use systype::{SysError, SysResult, SyscallResult};

use crate::{inode::Inode, File, InodeMode, Mutex, RenameFlags, SuperBlock};

pub struct DentryMeta {
    /// Name of this file or directory.
    pub name: String,
    pub super_block: Weak<dyn SuperBlock>,
    /// Parent dentry. `None` if root dentry.
    pub parent: Option<Weak<dyn Dentry>>,

    /// Inode it points to. May be `None`, which is called negative dentry.
    pub inode: Mutex<Option<Arc<dyn Inode>>>,
    /// Children dentries. Key value pair is <name, dentry>.
    // PERF: may be no need to be BTreeMap, since we will look up in hash table
    pub children: Mutex<BTreeMap<String, Arc<dyn Dentry>>>,
    pub state: Mutex<DentryState>,
}

impl DentryMeta {
    pub fn new(
        name: &str,
        super_block: Arc<dyn SuperBlock>,
        parent: Option<Arc<dyn Dentry>>,
    ) -> Self {
        log::debug!("[Dentry::new] new dentry with name {name}");
        let super_block = Arc::downgrade(&super_block);
        let inode = Mutex::new(None);
        if let Some(parent) = parent {
            Self {
                name: name.to_string(),
                super_block,
                inode,
                parent: Some(Arc::downgrade(&parent)),
                children: Mutex::new(BTreeMap::new()),
                state: Mutex::new(DentryState::UnInit),
            }
        } else {
            Self {
                name: name.to_string(),
                super_block,
                inode,
                parent: None,
                children: Mutex::new(BTreeMap::new()),
                state: Mutex::new(DentryState::UnInit),
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum DentryState {
    /// Either not read from disk or write in memory.
    UnInit,
    Sync,
    Dirty,
}

pub trait Dentry: Send + Sync {
    fn meta(&self) -> &DentryMeta;

    /// Open a file associated with the inode that this dentry points to.
    fn base_open(self: Arc<Self>) -> SysResult<Arc<dyn File>>;

    /// Look up in a directory inode and find file with `name`.
    ///
    /// If the named inode does not exist, a negative dentry will be created as
    /// a child and returned. Returning an error code from this routine must
    /// only be done on a real error.
    fn base_lookup(self: Arc<Self>, name: &str) -> SysResult<Arc<dyn Dentry>>;

    /// Called by the open(2) and creat(2) system calls. Create an inode for a
    /// dentry in the directory inode.
    ///
    /// If the dentry itself has a negative child with `name`, it will create an
    /// inode for the negative child and return the child.
    fn base_create(self: Arc<Self>, name: &str, mode: InodeMode) -> SysResult<Arc<dyn Dentry>>;

    /// Called by the unlink(2) system call. Delete a file inode in a directory
    /// inode.
    fn base_remove(self: Arc<Self>, name: &str) -> SysResult<()>;

    fn base_rename_to(self: Arc<Self>, new: Arc<dyn Dentry>, flags: RenameFlags) -> SysResult<()> {
        Err(SysError::EINVAL)
    }

    /// Create a negetive child dentry with `name`.
    fn base_new_child(self: Arc<Self>, _name: &str) -> Arc<dyn Dentry> {
        todo!()
    }

    fn inode(&self) -> SysResult<Arc<dyn Inode>> {
        self.meta()
            .inode
            .lock()
            .as_ref()
            .ok_or(SysError::ENOENT)
            .cloned()
    }

    fn super_block(&self) -> Arc<dyn SuperBlock> {
        self.meta().super_block.upgrade().unwrap()
    }

    fn name_string(&self) -> String {
        self.meta().name.clone()
    }

    fn name(&self) -> &str {
        &self.meta().name
    }

    fn parent(&self) -> Option<Arc<dyn Dentry>> {
        self.meta().parent.as_ref().map(|p| p.upgrade().unwrap())
    }

    fn children(&self) -> BTreeMap<String, Arc<dyn Dentry>> {
        self.meta().children.lock().clone()
    }

    fn get_child(&self, name: &str) -> Option<Arc<dyn Dentry>> {
        self.meta().children.lock().get(name).cloned()
    }

    fn remove_child(&self, name: &str) -> Option<Arc<dyn Dentry>> {
        self.meta().children.lock().remove(name)
    }

    fn set_inode(&self, inode: Arc<dyn Inode>) {
        if self.meta().inode.lock().is_some() {
            log::warn!("[Dentry::set_inode] replace inode in {:?}", self.name());
        }
        *self.meta().inode.lock() = Some(inode);
    }

    fn clear_inode(&self) {
        *self.meta().inode.lock() = None;
    }

    /// Insert a child dentry to this dentry.
    fn insert(&self, child: Arc<dyn Dentry>) -> Option<Arc<dyn Dentry>> {
        self.meta()
            .children
            .lock()
            .insert(child.name_string(), child)
    }

    fn change_state(&self, state: DentryState) {
        *self.meta().state.lock() = state;
    }

    /// Get the path of this dentry.
    // HACK: code looks ugly and may be has problem
    fn path(&self) -> String {
        if let Some(p) = self.parent() {
            let path = if self.name() == "/" {
                String::from("")
            } else {
                String::from("/") + self.name()
            };
            let parent_name = p.name();
            return if parent_name == "/" {
                if p.parent().is_some() {
                    // p is a mount point
                    p.parent().unwrap().path() + path.as_str()
                } else {
                    path
                }
            } else {
                // p is not root
                p.path() + path.as_str()
            };
        } else {
            log::warn!("dentry has no parent");
            String::from("/")
        }
    }
}

impl dyn Dentry {
    pub fn state(&self) -> DentryState {
        *self.meta().state.lock()
    }

    pub fn is_negetive(&self) -> bool {
        self.meta().inode.lock().is_none()
    }

    pub fn open(self: &Arc<Self>) -> SysResult<Arc<dyn File>> {
        self.clone().base_open()
    }

    pub fn lookup(self: &Arc<Self>, name: &str) -> SysResult<Arc<dyn Dentry>> {
        // let hash_key = HashKey::new(self, name)?;
        // if let Some(child) = dcache().get(hash_key) {
        //     log::warn!("[Dentry::lookup] find child in hash");
        //     return Ok(child);
        // }
        if !self.inode()?.itype().is_dir() {
            return Err(SysError::ENOTDIR);
        }
        let child = self.get_child_or_create(name);
        if child.state() == DentryState::UnInit {
            log::trace!(
                "[Dentry::lookup] lookup {name} not in cache in path {}",
                self.path()
            );
            self.clone().base_lookup(name)?;
            child.change_state(DentryState::Sync);
            return Ok(child);
        }
        Ok(child)
    }

    pub fn create(self: &Arc<Self>, name: &str, mode: InodeMode) -> SysResult<Arc<dyn Dentry>> {
        if !self.inode()?.itype().is_dir() {
            return Err(SysError::ENOTDIR);
        }
        let child = self.get_child_or_create(name);
        self.clone().base_create(name, mode)
    }

    pub fn remove(self: &Arc<Self>, name: &str) -> SysResult<()> {
        if !self.inode()?.itype().is_dir() {
            return Err(SysError::ENOTDIR);
        }
        let sub_dentry = self.get_child(name).ok_or(SysError::ENOENT)?;
        sub_dentry.clear_inode();
        self.clone().base_remove(name)
    }

    pub fn rename_to(self: &Arc<Self>, new: &Arc<Self>, flags: RenameFlags) -> SysResult<()> {
        if flags.contains(RenameFlags::RENAME_EXCHANGE)
            && (flags.contains(RenameFlags::RENAME_NOREPLACE)
                || flags.contains(RenameFlags::RENAME_WHITEOUT))
        {
            return Err(SysError::EINVAL);
        }
        if new.has_ancestor(self) {
            return Err(SysError::EINVAL);
        }

        if new.is_negetive() && flags.contains(RenameFlags::RENAME_EXCHANGE) {
            return Err(SysError::ENOENT);
        } else if flags.contains(RenameFlags::RENAME_NOREPLACE) {
            return Err(SysError::EEXIST);
        }
        self.clone().base_rename_to(new.clone(), flags)
    }

    /// Create a negetive child dentry with `name`.
    pub fn new_child(self: &Arc<Self>, name: &str) -> Arc<dyn Dentry> {
        let child = self.clone().base_new_child(name);
        // dcache().insert(child.clone());
        child
    }

    pub fn get_child_or_create(self: &Arc<Self>, name: &str) -> Arc<dyn Dentry> {
        self.get_child(name).unwrap_or_else(|| {
            let new_dentry = self.new_child(name);
            self.insert(new_dentry.clone());
            new_dentry
        })
    }

    pub fn has_ancestor(self: &Arc<Self>, dir: &Arc<Self>) -> bool {
        let mut parent_opt = self.parent();
        while let Some(parent) = parent_opt {
            if Arc::ptr_eq(self, dir) {
                return true;
            }
            parent_opt = parent.parent();
        }
        false
    }
}

impl<T: Send + Sync + 'static> Dentry for MaybeUninit<T> {
    fn meta(&self) -> &DentryMeta {
        todo!()
    }

    fn base_open(self: Arc<Self>) -> SysResult<Arc<dyn File>> {
        todo!()
    }

    fn base_lookup(self: Arc<Self>, _name: &str) -> SysResult<Arc<dyn Dentry>> {
        todo!()
    }

    fn base_create(self: Arc<Self>, _name: &str, _mode: InodeMode) -> SysResult<Arc<dyn Dentry>> {
        todo!()
    }

    fn base_remove(self: Arc<Self>, _name: &str) -> SysResult<()> {
        todo!()
    }

    fn path(&self) -> String {
        "no path".to_string()
    }
}

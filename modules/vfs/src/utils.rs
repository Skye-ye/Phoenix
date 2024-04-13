use crate::{dentry::VFSDentry, DEFAULT_PERMISSION_DIR, DEFAULT_PERMISSION_FILE, PERMISSION_LEN};

bitflags::bitflags! {
    // 文件权限
    pub struct VFSNodePermission: u16 {
        // 文件所有者拥有读权限
        const OWNER_READ = 0o100;
        // 文件所有者拥有写权限
        const OWNER_WRITE = 0o200;
        // 文件所有者拥有执行权限
        const OWNER_EXEC = 0o400;

        // 组用户拥有读权限
        const GROUP_READ = 0o10;
        // 组用户拥有写权限
        const GROUP_WRITE = 0o20;
        // 组用户拥有执行权限
        const GROUP_EXEC = 0o40;

        // 其他用户拥有读权限
        const OTHER_READ = 0o1;
        // 其他用户拥有写权限
        const OTHER_WRITE = 0o2;
        // 其他用户拥有执行权限
        const OTHER_EXEC = 0o4;
    }
}

impl From<&str> for VFSNodePermission {
    fn from(value: &str) -> Self {
        let bytes = value.as_bytes();
        assert_eq!(bytes.len(), PERMISSION_LEN);
        let mut perm = VFSNodePermission::empty();

        let perms = [
            (VfsNodePerm::OWNER_READ, b'r'),
            (VfsNodePerm::OWNER_WRITE, b'w'),
            (VfsNodePerm::OWNER_EXEC, b'x'),
            (VfsNodePerm::GROUP_READ, b'r'),
            (VfsNodePerm::GROUP_WRITE, b'w'),
            (VfsNodePerm::GROUP_EXEC, b'x'),
            (VfsNodePerm::OTHER_READ, b'r'),
            (VfsNodePerm::OTHER_WRITE, b'w'),
            (VfsNodePerm::OTHER_EXEC, b'x'),
        ];

        for (i, &(flag, ch)) in perms.iter().enumerate() {
            if bytes[i] == ch {
                perm |= flag;
            }
        }
        perm
    }
}

impl VFSNodePermission {
    // 将权限解析为一个长度为9的字符数组，由r, w, x, -组成
    pub const fn get_permission_self(&self) -> [u8; 9] {
        let mut perm = [b'-'; 9];
        let perms = [
            (VfsNodePerm::OWNER_READ, b'r'),
            (VfsNodePerm::OWNER_WRITE, b'w'),
            (VfsNodePerm::OWNER_EXEC, b'x'),
            (VfsNodePerm::GROUP_READ, b'r'),
            (VfsNodePerm::GROUP_WRITE, b'w'),
            (VfsNodePerm::GROUP_EXEC, b'x'),
            (VfsNodePerm::OTHER_READ, b'r'),
            (VfsNodePerm::OTHER_WRITE, b'w'),
            (VfsNodePerm::OTHER_EXEC, b'x'),
        ];

        for (i, &(flag, ch)) in perms.iter().enumerate() {
            if self.contains(flag) {
                perm[i] = ch;
            }
        }
        perm
    }

    // 返回文件默认权限，所有用户都可以读和写，但是不能执行
    pub const fn get_permission_file_default() -> Self {
        Self::from_bits_truncate(DEFAULT_PERMISSION_FILE)
    }

    pub const fn get_permission_dir_default() -> Self {
        Self::from_bits_truncate(DEFAULT_PERMISSION_DIR)
    }
}

// 文件与文件夹类型
#[repr(u8)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum VFSNodeType {
    // 未知类型
    Unknown = 0,
    // 先进先出类型（如管道）
    Fifo = 0o1,
    // 字符设备
    CharDevice = 0o2,
    // 文件夹
    Dir = 0o4,
    // 块设备
    BlockDevice = 0o6,
    // 普通文件
    File = 0o10,
    // 符号链接
    SymLink = 0o12,
    // 套接字
    Socket = 0o14,
}

impl From<u8> for VFSNodeType {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Unknown,
            0o1 => Self::Fifo,
            0o2 => Self::CharDevice,
            0o4 => Self::Dir,
            0o6 => Self::BlockDevice,
            0o10 => Self::File,
            0o12 => Self::SymLink,
            0o14 => Self::Socket,
            _ => Self::Unknown,
        }
    }
}

impl From<char> for VFSNodeType {
    fn from(value: char) -> Self {
        match value {
            '-' => Self::File,
            'd' => Self::Dir,
            'l' => Self::SymLink,
            'c' => Self::CharDevice,
            'b' => Self::BlockDevice,
            'p' => Self::Fifo,
            's' => Self::Socket,
            _ => Self::Unknown,
        }
    }
}

impl VFSNodeType {
    /// Tests whether this node type represents a regular file.
    pub const fn is_file(self) -> bool {
        matches!(self, Self::File)
    }

    /// Tests whether this node type represents a directory.
    pub const fn is_dir(self) -> bool {
        matches!(self, Self::Dir)
    }

    /// Tests whether this node type represents a symbolic link.
    pub const fn is_symlink(self) -> bool {
        matches!(self, Self::SymLink)
    }

    /// Returns `true` if this node type is a block device.
    pub const fn is_block_device(self) -> bool {
        matches!(self, Self::BlockDevice)
    }

    /// Returns `true` if this node type is a char device.
    pub const fn is_char_device(self) -> bool {
        matches!(self, Self::CharDevice)
    }

    /// Returns `true` if this node type is a fifo.
    pub const fn is_fifo(self) -> bool {
        matches!(self, Self::Fifo)
    }

    /// Returns `true` if this node type is a socket.
    pub const fn is_socket(self) -> bool {
        matches!(self, Self::Socket)
    }

    /// Returns a character representation of the node type.
    ///
    /// For example, `d` for directory, `-` for regular file, etc.
    pub const fn as_char(self) -> char {
        match self {
            Self::Fifo => 'p',
            Self::CharDevice => 'c',
            Self::Dir => 'd',
            Self::BlockDevice => 'b',
            Self::File => '-',
            Self::SymLink => 'l',
            Self::Socket => 's',
            _ => '?',
        }
    }
}

#[derive(Debug, Clone)]
pub struct VFSDirEntry {
    /// inode编号
    pub inode_num: u64,
    /// 文件类型
    pub ty: VFSNodeType,
    /// 文件名
    pub name: String,
}

bitflags! {
    /// ppoll 使用，表示对应在文件上等待或者发生过的事件
    pub struct VFSPollEvents: u16 {
        /// 可读
        const IN = 0x0001;
        /// 可写
        const OUT = 0x0004;
        /// 报错
        const ERR = 0x0008;
        /// 已终止，如 pipe 的另一端已关闭连接的情况
        const HUP = 0x0010;
        /// 无效的 fd
        const INVAL = 0x0020;
    }
}

#[derive(Clone)]
pub struct VFSMountPoint {
    pub root: Arc<dyn VFSDentry>,
    pub mount_point: Weak<dyn VFSDentry>,
    pub mount_flags: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy, Debug, Eq, PartialEq)]
pub struct VFSTimeSpec {
    pub sec: u64,  // 秒
    pub nsec: u64, // 纳秒, 范围在0~999999999
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
#[repr(C)]
pub struct VFSFileStat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub __pad: u64,
    pub st_size: u64,
    pub st_blksize: u32,
    pub __pad2: u32,
    pub st_blocks: u64,
    pub st_atime: VFSTimeSpec,
    pub st_mtime: VFSTimeSpec,
    pub st_ctime: VFSTimeSpec,
    pub unused: u64,
}

bitflags! {
    /// renameat flag
   pub struct VFSRenameFlag: u32 {
       /// Atomically exchange oldpath and newpath.
       /// Both pathnames must exist but may be of different type
       const RENAME_EXCHANGE = 1 << 1;
       /// Don't overwrite newpath of the rename. Return an error if newpath already exists.
       const RENAME_NOREPLACE = 1 << 0;
       /// This operation makes sense only for overlay/union filesystem implementations.
       const RENAME_WHITEOUT = 1 << 2;
   }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum VFSTime {
    AccessTime(VFSTimeSpec),
    ModifiedTime(VFSTimeSpec),
}

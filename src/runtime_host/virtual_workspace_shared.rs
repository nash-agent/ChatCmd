use super::*;
use sha2::{Digest, Sha256};

impl VirtualWorkspaceView {
    pub(in crate::runtime_host) fn with_shared_roots(mut self, shared: Vec<PathBuf>) -> Self {
        for physical in shared {
            if let Some(root) = self
                .roots
                .iter()
                .find(|root| same_path(&root.physical, &physical))
            {
                self.shared_aliases.push(root.alias.clone());
                continue;
            }
            let hash = format!("{:x}", Sha256::digest(path_key(&physical).as_bytes()));
            let alias = format!("@shared-{}-{}", alias_base(&physical), &hash[..12]);
            // Never remap an occupied alias to a different root.
            if self.roots.iter().any(|root| root.alias == alias) {
                continue;
            }
            self.shared_aliases.push(alias.clone());
            self.roots.push(VirtualRoot::new(alias, physical));
        }
        self
    }
}

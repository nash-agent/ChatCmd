# Workspace path safety and traversal policy

Filesystem requests are authorized after canonicalization. An absolute path is not an implicit
grant: it must be inside a configured workspace root, an explicitly saved all-conversation root,
or an explicit path scope derived from the current task's user message. A file scope grants only
that file; a directory scope grants its subtree. A filesystem root is accepted only when it was
explicitly selected and saved as an all-conversation project root. Merely mentioning a drive root
in a conversation does not create that broad grant.

Source and destination paths are resolved independently. Runtime operations carry an internal
validated capability containing the canonical path or parent, authorized root, entry kind, access
intent, and a best-effort filesystem identity. The identity and parent are checked again immediately
before blocking reads or mutations.

A directly requested symbolic link, junction, or other Windows reparse path is canonicalized first.
The resolved target must still be inside an authorized scope. Runtime operations then use that
canonical target, so retargeting the original link cannot redirect an already-authorized operation.
Destructive operations do not act through a final link entry, and recursive walkers do not blindly
follow nested links.

## Platform guarantees

- Unix records device/inode plus size and modification time. Requested links used as path
  components are resolved to a canonical target and that target must remain inside an authorized
  scope. The implementation revalidates before use, but portable path-based syscalls still leave a
  narrow race window; it does not claim `openat2`-level race freedom.
- Windows treats junctions and other reparse path components the same way for direct requests:
  resolve first, then enforce the authorized scope on the canonical target. Identity revalidation
  uses creation time, size, and modification time because stable `std` does not expose a portable
  held directory-handle workflow. It is best-effort rather than fully race-free.
- macOS follows the Unix policy. Filesystem Unicode normalization and case behavior remain those of
  the mounted filesystem; authorization always compares canonical paths.

## Shared ignore precedence

Search, find, and the turn file-change watcher share one default generated-directory list. Walkers
respect nested `.gitignore` rules (including negation) unless `includeIgnored` is enabled. Explicit
exclude patterns always win, including when ignored files are otherwise included. The requested
root itself is always visited, even when its name appears in the default list, so a direct-root
request remains usable. Recursive walkers do not blindly follow symbolic links and traversal depth
is bounded; callers can still address a link directly, in which case canonical target scope checks
apply.

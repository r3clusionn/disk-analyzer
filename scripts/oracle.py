"""Independent size oracle for checking dux.

    python scripts/oracle.py PATH

Walks PATH with os.scandir (no symlink or junction following), sums file lengths, and counts a
hard-linked file once by its (device, inode) pair. Prints the same totals dux prints, so the two
can be compared. Written separately from the Rust code on purpose.
"""
import os
import stat
import sys


def main(root):
    seen = set()
    total = files = dups = errors = 0
    stack = [root]
    while stack:
        d = stack.pop()
        try:
            it = os.scandir(d)
        except OSError:
            errors += 1
            continue
        with it:
            for e in it:
                try:
                    st = e.stat(follow_symlinks=False)
                except OSError:
                    errors += 1
                    continue
                mode = st.st_mode
                is_reparse = getattr(st, "st_file_attributes", 0) & 0x400
                if stat.S_ISLNK(mode) or (is_reparse and stat.S_ISDIR(mode)):
                    continue  # links are not followed and add no size
                if stat.S_ISDIR(mode):
                    stack.append(e.path)
                    continue
                files += 1
                # On Windows DirEntry.stat() leaves st_ino and st_nlink at zero; os.stat fills them.
                try:
                    st = os.stat(e.path)
                except OSError:
                    errors += 1
                    continue
                if st.st_nlink > 1:
                    key = (st.st_dev, st.st_ino)
                    if key in seen:
                        dups += 1
                        continue
                    seen.add(key)
                total += st.st_size
    print(f"bytes={total} files={files} hardlink_duplicates={dups} errors={errors}")


if __name__ == "__main__":
    main(sys.argv[1])

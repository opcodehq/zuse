package sh.zuse.core

import kotlin.test.*

class GitPatchTests {
    @Test
    fun `matches complete paths quoted Unicode renames and deleted files`() {
        val ordinary = "diff --git a/file b/file\n--- a/file\n+++ b/file\n"
        val other = "diff --git a/myfile b/myfile\n"
        assertEquals(ordinary, GitPatch.file(other + ordinary, "file"))
        assertNull(GitPatch.file(other, "file"))
        val renamed = "diff --git a/old name b/new name\nrename from old name\nrename to new name\n"
        assertEquals(renamed, GitPatch.file(renamed, "new name", "old name"))
        val quoted = "diff --git \"a/\\303\\251.txt\" \"b/\\303\\251.txt\"\n"
        assertEquals(quoted, GitPatch.file(quoted, "é.txt"))
        val mixed = "diff --git \"a/é\\t.txt\" \"b/é\\t.txt\"\n"
        assertEquals(mixed, GitPatch.file(mixed, "é\t.txt"))
        val deleted = "diff --git a/gone b/gone\n--- a/gone\n+++ /dev/null\n"
        assertEquals(deleted, GitPatch.file(deleted, "gone"))
    }
}

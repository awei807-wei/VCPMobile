package com.vcp.mobile

import java.io.File
import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class CanonicalPathGuardTest {
    @Test
    fun `校验后返回同一个规范文件对象避免原路径 TOCTOU`() {
        val allowed = File("/data/cache")
        val verified = resolveCanonicalFileInDirectories(
            File("/data/cache/../cache/uploads/image.png"),
            listOf(allowed),
        )

        assertEquals("/data/cache/uploads/image.png", verified?.path)
        assertNull(
            resolveCanonicalFileInDirectories(
                File("/data/cache-old/image.png"),
                listOf(allowed),
            ),
        )
    }

    @Test
    fun `分享文件路径守卫拒绝越界路径并返回规范普通文件`() {
        val root = Files.createTempDirectory("vcp-share-guard").toFile()
        try {
            val cache = File(root, "cache").apply { mkdirs() }
            val shared = File(cache, "shared").apply { mkdirs() }
            val valid = File(shared, "valid.txt").apply { writeText("ok") }
            val cacheSibling = File(cache, "outside.txt").apply { writeText("no") }
            val adjacent = File(root, "cache-old").apply { mkdirs() }
                .resolve("adjacent.txt")
                .apply { writeText("no") }
            val otherSandbox = File(root, "files/other.txt").apply {
                parentFile?.mkdirs()
                writeText("no")
            }

            assertEquals(
                valid.canonicalFile,
                resolveCanonicalRegularFileInDirectory(
                    File(shared, "nested/../valid.txt"),
                    shared,
                ),
            )
            assertNull(
                resolveCanonicalRegularFileInDirectory(
                    File(shared, "../outside.txt"),
                    shared,
                ),
            )
            assertNull(resolveCanonicalRegularFileInDirectory(adjacent, shared))
            assertNull(resolveCanonicalRegularFileInDirectory(otherSandbox, shared))

            val symlink = File(shared, "linked.txt")
            Files.createSymbolicLink(symlink.toPath(), cacheSibling.toPath())
            assertNull(resolveCanonicalRegularFileInDirectory(symlink, shared))
        } finally {
            root.deleteRecursively()
        }
    }
}

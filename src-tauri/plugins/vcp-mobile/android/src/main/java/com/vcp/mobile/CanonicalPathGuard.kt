package com.vcp.mobile

import java.io.File

/** 判断规范化路径是否位于指定目录本身或其子目录中。 */
internal fun isWithinCanonicalDirectory(
    candidatePath: String,
    directoryPath: String,
): Boolean {
    val directory = directoryPath.trimEnd(File.separatorChar)
    if (directory.isEmpty()) return candidatePath.startsWith(File.separator)
    return candidatePath == directory ||
        candidatePath.startsWith(directory + File.separator)
}

/** 仅返回已完成规范化且位于允许目录内的文件，调用方后续必须复用该对象。 */
internal fun resolveCanonicalFileInDirectories(
    candidate: File,
    allowedDirectories: List<File>,
): File? {
    val canonicalCandidate = try {
        candidate.canonicalFile
    } catch (_: Exception) {
        return null
    }
    return canonicalCandidate.takeIf { file ->
        allowedDirectories.any { directory ->
            isWithinCanonicalDirectory(file.path, directory.path)
        }
    }
}

/** 返回允许目录内已规范化且确认为普通文件的对象，供异步任务继续复用。 */
internal fun resolveCanonicalRegularFileInDirectory(
    candidate: File,
    allowedDirectory: File,
): File? {
    val canonicalDirectory = try {
        allowedDirectory.canonicalFile
    } catch (_: Exception) {
        return null
    }
    return resolveCanonicalFileInDirectories(candidate, listOf(canonicalDirectory))
        ?.takeIf { it.isFile }
}

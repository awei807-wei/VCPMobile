package com.vcp.mobile.service

/** 纯 JVM 可测的前台流 lease 账本。每次 acquire 都分配单调 generation。 */
class ForegroundLeaseLedger {
    private val generations = HashMap<String, LinkedHashSet<Long>>()
    private var nextGeneration = 0L

    @Synchronized
    fun acquire(tag: String): Long {
        val generation = ++nextGeneration
        generations.getOrPut(tag) { LinkedHashSet() }.add(generation)
        return generation
    }

    @Synchronized
    fun release(tag: String, expectedGeneration: Long? = null): Boolean {
        return releaseGeneration(tag, expectedGeneration) != null
    }

    @Synchronized
    fun releaseGeneration(tag: String, expectedGeneration: Long? = null): Long? {
        val active = generations[tag] ?: return null
        val removed = if (expectedGeneration == null) {
            active.firstOrNull()?.also(active::remove)
        } else {
            expectedGeneration.takeIf { active.remove(it) }
        }
        if (active.isEmpty()) {
            generations.remove(tag)
        }
        return removed
    }

    @Synchronized
    fun isHeld(tag: String): Boolean = !generations[tag].isNullOrEmpty()

    @Synchronized
    fun clear() {
        generations.clear()
    }
}

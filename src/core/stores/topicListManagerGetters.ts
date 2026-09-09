import { computed, type Ref } from "vue";
import type { Topic } from "./topicTypes";

export function createFilteredTopics(
  topics: Ref<Topic[]>,
  searchTerm: Ref<string>,
) {
  return computed(() => {
    const term = searchTerm.value.toLowerCase().trim();
    if (!term) return topics.value;
    return topics.value.filter((topic) => {
      const nameMatch = topic.name.toLowerCase().includes(term);
      let dateMatch = false;
      const createdAt = (topic as any).createdAt || (topic as any).created_at;
      if (createdAt) {
        const date = new Date(createdAt > 1e11 ? createdAt : createdAt * 1000);
        const fullDate = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")} ${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
        const shortDate = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
        dateMatch = fullDate.includes(term) || shortDate.includes(term);
      }
      return nameMatch || dateMatch;
    });
  });
}

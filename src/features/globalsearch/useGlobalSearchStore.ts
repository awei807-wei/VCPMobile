import { defineStore } from "pinia";
import { GlobalSearchController } from "./searchController";

export const useGlobalSearchStore = defineStore("globalSearch", () => {
  const controller = new GlobalSearchController();
  return controller.expose();
});

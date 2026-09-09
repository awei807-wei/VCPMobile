export interface SharedFileEntry {
  cachePath: string;
  mimeType: string;
  fileName: string;
  size: number;
}

export interface SharedContentData {
  text: string;
  files: SharedFileEntry[];
}

export interface PickedFileInfo {
  path: string;
  name: string;
  mime: string;
  size: number;
  hash: string;
  thumbnailPath?: string;
}

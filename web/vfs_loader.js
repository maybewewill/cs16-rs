const GOLDSRC_LAYERS = new Set([
  'cstrike',
  'cstrike_hd',
  'cstrike_downloads',
  'valve',
  'platform',
  'mods'
]);

const ALLOWED_EXT = new Set([
  'bsp', 'wad', 'mdl', 'spr', 'wav', 'mp3', 'tga', 'bmp', 'res', 'txt', 'cfg', 'sc', 'lst', 'vdf', 'db'
]);

const DB_NAME = 'cs16-rs-assets-v1';
const STORE_NAME = 'files';

function wantsAsset(relPath) {
  const lower = relPath.replace(/\\/g, '/').toLowerCase();
  const segments = lower.split('/').filter(Boolean);
  const inLayer = segments.some((s) => GOLDSRC_LAYERS.has(s));
  if (!inLayer && segments.length > 2) return false;
  const dot = lower.lastIndexOf('.');
  if (dot < 0) return false;
  return ALLOWED_EXT.has(lower.slice(dot + 1));
}

export async function mountFromDirectoryPicker(onFile, onProgress) {
  if (typeof window.showDirectoryPicker !== 'function') {
    throw new Error('File System Access API (showDirectoryPicker) is unavailable in this browser');
  }
  const dirHandle = await window.showDirectoryPicker({ mode: 'read' });
  const mounted = [];
  let scanned = 0;

  async function walk(handle, prefix) {
    for await (const entry of handle.values()) {
      const path = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.kind === 'directory') {
        await walk(entry, path);
      } else if (entry.kind === 'file' && wantsAsset(path)) {
        const file = await entry.getFile();
        const bytes = new Uint8Array(await file.arrayBuffer());
        await onFile(path, bytes);
        mounted.push({ path, bytes });
        scanned += 1;
        if (onProgress && (scanned % 25 === 0)) {
          onProgress(scanned, path);
        }
      }
    }
  }

  await walk(dirHandle, dirHandle.name);
  if (onProgress) onProgress(scanned, 'complete');
  return mounted;
}

export async function mountFromFileList(fileList, onFile, onProgress) {
  const mounted = [];
  let scanned = 0;
  for (const file of fileList) {
    const path = file.webkitRelativePath || file.name;
    if (!wantsAsset(path)) continue;
    const bytes = new Uint8Array(await file.arrayBuffer());
    await onFile(path, bytes);
    mounted.push({ path, bytes });
    scanned += 1;
    if (onProgress && (scanned % 25 === 0)) {
      onProgress(scanned, path);
    }
  }
  if (onProgress) onProgress(scanned, 'complete');
  return mounted;
}

export async function mountFromDropEvent(event, onFile, onProgress) {
  const items = [...(event.dataTransfer?.items || [])];
  const entries = [];
  for (const item of items) {
    if (typeof item.webkitGetAsEntry === 'function') {
      const entry = item.webkitGetAsEntry();
      if (entry) entries.push(entry);
    }
  }
  if (entries.length === 0) {
    return mountFromFileList(event.dataTransfer?.files || [], onFile, onProgress);
  }

  const mounted = [];
  let scanned = 0;

  async function readEntry(entry, prefix) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isFile) {
      if (!wantsAsset(path)) return;
      const file = await new Promise((res, rej) => entry.file(res, rej));
      const bytes = new Uint8Array(await file.arrayBuffer());
      await onFile(path, bytes);
      mounted.push({ path, bytes });
      scanned += 1;
      if (onProgress && (scanned % 25 === 0)) {
        onProgress(scanned, path);
      }
    } else if (entry.isDirectory) {
      const reader = entry.createReader();
      while (true) {
        const batch = await new Promise((res, rej) => reader.readEntries(res, rej));
        if (!batch || batch.length === 0) break;
        for (const child of batch) {
          await readEntry(child, path);
        }
      }
    }
  }

  for (const entry of entries) {
    await readEntry(entry, '');
  }
  if (onProgress) onProgress(scanned, 'complete');
  return mounted;
}

function openDb() {
  return new Promise((resolve, reject) => {
    if (typeof indexedDB === 'undefined') {
      reject(new Error('IndexedDB unavailable'));
      return;
    }
    const req = indexedDB.open(DB_NAME, 1);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME, { keyPath: 'path' });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

export async function cacheAssetsInDb(entries) {
  if (!entries || entries.length === 0) return;
  const db = await openDb();
  await new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, 'readwrite');
    const store = tx.objectStore(STORE_NAME);
    for (const item of entries) {
      store.put({ path: item.path, bytes: item.bytes });
    }
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
  });
  db.close();
}

export async function hydrateAssetsFromDb(onFile, onProgress) {
  let db;
  try {
    db = await openDb();
  } catch {
    return 0;
  }
  const count = await new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, 'readonly');
    const store = tx.objectStore(STORE_NAME);
    const req = store.openCursor();
    let total = 0;
    req.onsuccess = () => {
      const cursor = req.result;
      if (cursor) {
        const { path, bytes } = cursor.value;
        onFile(path, bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes));
        total += 1;
        if (onProgress && total % 50 === 0) onProgress(total, path);
        cursor.continue();
      } else {
        resolve(total);
      }
    };
    req.onerror = () => reject(req.error);
  });
  db.close();
  if (onProgress) onProgress(count, 'cached');
  return count;
}

export async function clearAssetDb() {
  try {
    const db = await openDb();
    await new Promise((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, 'readwrite');
      tx.objectStore(STORE_NAME).clear();
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
    db.close();
  } catch {
    // Ignore when IndexedDB is unavailable in private windows.
  }
}

import * as ipc from './ipc';

/**
 * Maps action-wheel segment ids to the backend's `run_action` command. Only
 * `copy` and `move` touch the user's files, and both first ask for a
 * destination folder; cancelling that dialog does nothing.
 */
export async function performAction(id: string, paths: string[]): Promise<void> {
  if (paths.length === 0) return;
  if (id === 'copy' || id === 'move') {
    const dest = await ipc.pickFolder();
    if (!dest) return;
    await ipc.runAction(id, paths, dest);
    return;
  }
  await ipc.runAction(id, paths);
}

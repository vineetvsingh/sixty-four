// Search worker: loads the same WebAssembly module as the page and runs engine
// searches off the main thread. The handler is installed synchronously so that
// requests posted while the module is still loading are queued, not lost.
import init, { worker_search } from './pkg/app.js';

const ready = init();

onmessage = async (e) => {
  await ready;
  postMessage(worker_search(e.data));
};

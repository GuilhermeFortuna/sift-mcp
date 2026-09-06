import { createRoot } from 'react-dom/client';
import { session } from './api/client';
void session().catch(() => undefined);
createRoot(document.getElementById('root')!).render(<main><h1>Sift Console</h1><p>Local service connected.</p></main>);

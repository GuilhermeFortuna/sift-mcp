import { createRoot } from 'react-dom/client';
import { useEffect, useState } from 'react';
import { session } from './api/client';
import { AppShell } from './components/AppShell';
import { RepositoriesPage } from './pages/RepositoriesPage';
import { RepositoryDetailPage } from './pages/RepositoryDetailPage';
import { RepositoryFormPage } from './pages/RepositoryFormPage';
import './styles.css';

function Router() { const [path, setPath] = useState(window.location.pathname); useEffect(() => { const onPop = () => setPath(window.location.pathname); window.addEventListener('popstate', onPop); return () => window.removeEventListener('popstate', onPop); }, []); useEffect(() => { document.title = path.startsWith('/repositories/') ? 'Repository · Sift Console' : 'Repositories · Sift Console'; }, [path]); if (path === '/repositories/new') return <RepositoryFormPage />; const match = path.match(/^\/repositories\/([^/]+)(\/edit)?$/); if (match) return match[2] ? <RepositoryFormPage id={match[1]} /> : <RepositoryDetailPage id={match[1]} />; return <RepositoriesPage />; }
void session().catch(() => undefined);
createRoot(document.getElementById('root')!).render(<AppShell><Router /></AppShell>);

import type { ReactNode } from 'react';

export function AppShell({ children }: { children: ReactNode }) {
  const path = typeof window !== 'undefined' ? window.location.pathname : '';
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">S</span>
          <span>Sift Console</span>
        </div>
        <p className="eyebrow">Local code intelligence</p>
        <nav aria-label="Primary navigation">
          <a className={path === '/' ? 'active' : ''} href="/">Overview</a>
          <a className={path.startsWith('/activity') ? 'active' : ''} href="/activity">Activity</a>
          <a className={path.startsWith('/repositories') ? 'active' : ''} href="/repositories">Repositories</a>
          <a className={path.startsWith('/search') ? 'active' : ''} href="/search">Search Lab</a>
        </nav>
        <div className="sidebar-note">
          <span className="status-dot" /> Loopback service<br />
          <small>127.0.0.1 · metadata only</small>
        </div>
      </aside>
      <main className="main-content">{children}</main>
    </div>
  );
}

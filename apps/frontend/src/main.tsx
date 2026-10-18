import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './styles.css';
import './capture.css';
import './split.css';
import './context-menu.css';
import './response.css';
import './workbench.css';
import './theme.css';
import './request-details.css';
import './fields.css';
import './secondary-workspaces.css';
import './ui-refresh.css';
import './cyberpunk.css';
import './extra-themes.css';
import './expressive-themes.css';
import { applyTheme, readTheme } from './theme';

applyTheme(readTheme());

ReactDOM.createRoot(document.getElementById('root')!).render(<React.StrictMode><App /></React.StrictMode>);

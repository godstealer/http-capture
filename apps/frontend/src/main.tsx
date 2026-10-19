import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import ConfigurationStartup from './ConfigurationStartup';
import {reloadLanguagePreference} from './i18n';
import {reloadBodyEditorPreferences} from './bodyEditorPreferences';
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
function refreshImportedPreferences(){applyTheme(readTheme());reloadLanguagePreference();reloadBodyEditorPreferences()}

ReactDOM.createRoot(document.getElementById('root')!).render(<React.StrictMode><ConfigurationStartup onReady={refreshImportedPreferences}>{status=><App initialImportStatus={status}/>}</ConfigurationStartup></React.StrictMode>);

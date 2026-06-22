const fs = require('fs');
const content = fs.readFileSync('src/App.vue', 'utf8');

const styleStart = content.indexOf('<style scoped>');
const styleContent = content.substring(styleStart);

const newTemplateAndScript = `
<script setup>
import { ref, computed } from 'vue';
import { useI18n } from 'vue-i18n';
import LiquidFilter from './components/LiquidFilter.vue';

const { t, locale } = useI18n();

const toggleLanguage = () => {
  locale.value = locale.value === 'en' ? 'zh' : 'en';
};

const closeWindow = () => window.__TAURI_INTERNALS__.invoke('plugin:window|close');
const minimizeWindow = () => window.__TAURI_INTERNALS__.invoke('plugin:window|minimize');
const maximizeWindow = () => window.__TAURI_INTERNALS__.invoke('plugin:window|toggle_maximize');

// MFB Options
const optimizeMode = ref('adjacent');

const triggerAction = (tool) => {
  console.log('Action triggered:', tool);
};
</script>

<template>
  <div class="app" data-theme="dark">
    <LiquidFilter />
    <!-- Header -->
    <header class="header liquid-glass">
      <div class="header-left">
        <div class="logo-container">
          <div class="logo-icon">🚀</div>
          <div class="logo-glow"></div>
        </div>
        <div class="title-group">
          <h1>Modern Format Boost</h1>
          <p>Next-Gen Media Engine 🔮</p>
        </div>
      </div>
      <div class="header-right">
        <div class="action-group">
          <button class="icon-btn" @click="toggleLanguage" title="Language">
            <span class="text-icon">{{ locale === 'zh' ? 'CN' : 'EN' }}</span>
          </button>
        </div>
        
        <!-- Window Controls -->
        <div class="window-controls">
          <button class="window-btn minimize" @click="minimizeWindow">
            <svg width="10" height="10" viewBox="0 0 12 12"><rect x="2" y="5" width="8" height="2" fill="currentColor"/></svg>
          </button>
          <button class="window-btn maximize" @click="maximizeWindow">
            <svg width="10" height="10" viewBox="0 0 12 12"><rect x="2" y="2" width="8" height="8" fill="none" stroke="currentColor" stroke-width="1.5"/></svg>
          </button>
          <button class="window-btn close" @click="closeWindow">
            <svg width="10" height="10" viewBox="0 0 12 12"><path d="M2 2 L10 10 M10 2 L2 10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
          </button>
        </div>
      </div>
    </header>

    <!-- Main -->
    <main class="main">
      <div class="content fade-in">
        <!-- Left: Controls -->
        <div class="panel controls-panel glass">
          <h3 class="panel-title">
            <span class="icon">⚙️</span> Optimization Mode
          </h3>

          <div class="form-group">
            <div class="select-wrapper">
              <select v-model="optimizeMode" class="select">
                <option value="adjacent">Output to Adjacent Folder</option>
                <option value="inplace">In-Place Optimization</option>
                <option value="fastvideo">Fast Video Mode</option>
                <option value="fastimg">Fast Image Mode</option>
              </select>
            </div>
          </div>

          <details class="details glass-panel" open>
            <summary>Workspace Tools</summary>
            <div class="checkbox-group">
              <button class="btn btn-secondary btn-block" style="margin-bottom:8px" @click="triggerAction('collect')">Collect Optimized Media</button>
              <button class="btn btn-secondary btn-block" style="margin-bottom:8px" @click="triggerAction('merge')">Merge XMP Attachments</button>
              <button class="btn btn-secondary btn-block" @click="triggerAction('icloud')">iCloud Photo Import</button>
            </div>
          </details>

          <details class="details glass-panel" open>
            <summary>Maintenance Tools</summary>
            <div class="checkbox-group">
              <button class="btn btn-secondary btn-block" style="margin-bottom:8px" @click="triggerAction('diag')">Diagnostic Analysis</button>
              <button class="btn btn-secondary btn-block" style="margin-bottom:8px" @click="triggerAction('clean')">Cleanup Cache & Logs</button>
              <button class="btn btn-secondary btn-block" @click="triggerAction('db')">Database Manager</button>
            </div>
          </details>
          
          <div style="flex:1"></div>

          <button class="btn btn-primary btn-block btn-lg">
            🔮 Start Engine
          </button>
        </div>

        <!-- Right: File List Area -->
        <div class="panel files-panel glass">
          <div class="empty-inline fade-in" style="height:100%; display:flex; align-items:center; justify-content:center;">
             <div class="empty-inline-content" style="text-align:center">
               <span class="empty-inline-icon" style="font-size:4rem">📂</span>
               <h3 style="margin-top:20px; color:var(--color-text-secondary)">Drag and Drop Media Folders Here</h3>
             </div>
          </div>
        </div>
      </div>
    </main>
  </div>
</template>

`;

fs.writeFileSync('src/App.vue', newTemplateAndScript + styleContent);
console.log('App.vue updated successfully.');

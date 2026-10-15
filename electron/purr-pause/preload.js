const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('electronAPI', {
  onStartAnimation: (callback) => {
    ipcRenderer.removeAllListeners('start-animation');
    ipcRenderer.on('start-animation', (event, config) => callback(config));
  },
  overlayShown: () => ipcRenderer.send('overlay-shown'),
  dismiss: () => ipcRenderer.send('dismissed'),
  snooze: () => ipcRenderer.send('snooze'),
  onLoadConfig: (callback) => {
    ipcRenderer.removeAllListeners('load-config');
    ipcRenderer.on('load-config', (event, config) => callback(config));
  },
  saveConfig: (config) => ipcRenderer.send('save-config', config),
  downloadMessageApiSpec: () => ipcRenderer.send('download-message-api-spec'),
  onDownloadMessageApiSpecResult: (callback) => {
    ipcRenderer.removeAllListeners('download-message-api-spec-result');
    ipcRenderer.on('download-message-api-spec-result', (event, result) => callback(result));
  },
  onSaveConfigResult: (callback) => {
    ipcRenderer.removeAllListeners('save-config-result');
    ipcRenderer.on('save-config-result', (event, result) => callback(result));
  },
  closeSettings: () => ipcRenderer.send('close-settings'),
  pickWebmDir: () => ipcRenderer.send('pick-webm-dir'),
  onWebmDirPicked: (callback) => {
    ipcRenderer.removeAllListeners('webm-dir-picked');
    ipcRenderer.on('webm-dir-picked', (event, dir) => callback(dir));
  },
  activate: (serial) => ipcRenderer.send('activate', serial),
  skipActivation: () => ipcRenderer.send('skip-activation'),
  onActivationResult: (callback) => {
    ipcRenderer.removeAllListeners('activation-result');
    ipcRenderer.on('activation-result', (event, result) => callback(result));
  },
  onLicenseStatus: (callback) => {
    ipcRenderer.removeAllListeners('license-status');
    ipcRenderer.on('license-status', (event, status) => callback(status));
  },
  onMessagesState: (callback) => {
    ipcRenderer.removeAllListeners('messages-state');
    ipcRenderer.on('messages-state', (event, state) => callback(state));
  },
  refreshMessages: () => ipcRenderer.send('refresh-messages'),
  openMessageTarget: (id) => ipcRenderer.send('open-message-target', id),
  dismissMessageNotification: () => ipcRenderer.send('dismiss-message-notification'),
  testMessageConnection: (config) => ipcRenderer.send('test-message-connection', config),
  onTestMessageConnectionResult: (callback) => {
    ipcRenderer.removeAllListeners('test-message-connection-result');
    ipcRenderer.on('test-message-connection-result', (event, result) => callback(result));
  }
});

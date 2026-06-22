/**
 * 统一日志系统
 * 使用键名标识，便于后期过滤和分析
 */

const LOG_LEVELS = {
  ERROR: 'ERROR',
  WARN: 'WARN',
  INFO: 'INFO',
  DEBUG: 'DEBUG'
}

const LOG_KEYS = {
  // 应用生命周期
  APP_INIT: 'app.init',
  APP_MOUNT: 'app.mount',
  APP_ERROR: 'app.error',
  
  // 文件操作
  FILE_LOAD: 'file.load',
  FILE_LOAD_SUCCESS: 'file.load.success',
  FILE_LOAD_ERROR: 'file.load.error',
  FILE_REMOVE: 'file.remove',
  
  // 转换操作
  CONVERT_START: 'convert.start',
  CONVERT_PROGRESS: 'convert.progress',
  CONVERT_SUCCESS: 'convert.success',
  CONVERT_ERROR: 'convert.error',
  
  // Rust CLI
  RUST_CLI_EXEC: 'rust.cli.exec',
  RUST_CLI_STDOUT: 'rust.cli.stdout',
  RUST_CLI_STDERR: 'rust.cli.stderr',
  RUST_CLI_ERROR: 'rust.cli.error',
  
  // Eagle API
  EAGLE_API_CALL: 'eagle.api.call',
  EAGLE_API_SUCCESS: 'eagle.api.success',
  EAGLE_API_ERROR: 'eagle.api.error',
  
  // 参数变更
  PARAM_CHANGE: 'param.change',
  FORMAT_CHANGE: 'format.change',
  QUALITY_CHANGE: 'quality.change',
  
  // UI交互
  UI_CLICK: 'ui.click',
  UI_EXPAND: 'ui.expand',
  UI_COLLAPSE: 'ui.collapse'
}

class Logger {
  constructor() {
    this.enabled = true
    this.level = LOG_LEVELS.INFO
  }

  log(level, key, message, data = {}) {
    if (!this.enabled) return
    
    const timestamp = new Date().toISOString()
    const logEntry = {
      timestamp,
      level,
      key,
      message,
      ...data
    }

    const prefix = `[PIXLY ${level}] [${key}]`
    
    switch (level) {
      case LOG_LEVELS.ERROR:
        console.error(prefix, message, data)
        break
      case LOG_LEVELS.WARN:
        console.warn(prefix, message, data)
        break
      case LOG_LEVELS.INFO:
        console.info(prefix, message, data)
        break
      case LOG_LEVELS.DEBUG:
        console.debug(prefix, message, data)
        break
    }

    return logEntry
  }

  error(key, message, data) {
    return this.log(LOG_LEVELS.ERROR, key, message, data)
  }

  warn(key, message, data) {
    return this.log(LOG_LEVELS.WARN, key, message, data)
  }

  info(key, message, data) {
    return this.log(LOG_LEVELS.INFO, key, message, data)
  }

  debug(key, message, data) {
    return this.log(LOG_LEVELS.DEBUG, key, message, data)
  }

  setLevel(level) {
    this.level = level
  }

  enable() {
    this.enabled = true
  }

  disable() {
    this.enabled = false
  }
}

// 单例
const logger = new Logger()

export { logger, LOG_KEYS, LOG_LEVELS }

#!/usr/bin/env node

/**
 * ============================================================================
 * 🔌 JSON API 调用示例 (Node.js)
 * ============================================================================
 * 
 * 演示如何在Node.js中调用imgquality的JSON API
 * 
 * Usage: node json_api.js <image_file>
 * ============================================================================
 */

const { execSync } = require('child_process');
const fs = require('fs');

// 检查命令行参数
if (process.argv.length < 3) {
    console.error('❌ Error: No image file specified');
    console.error('Usage: node json_api.js <image_file>');
    process.exit(1);
}

const imageFile = process.argv[2];

// 检查文件是否存在
if (!fs.existsSync(imageFile)) {
    console.error(`❌ Error: File not found: ${imageFile}`);
    process.exit(1);
}

console.log('╔══════════════════════════════════════════════╗');
console.log('║   🔌 imgquality JSON API Demo                ║');
console.log('╚══════════════════════════════════════════════╝');
console.log('');

try {
    // 调用 imgquality 并获取 JSON 输出
    console.log('📡 Calling imgquality API...');
    const output = execSync(
        `imgquality analyze "${imageFile}" --output json --recommend`,
        { encoding: 'utf8' }
    );

    // 解析 JSON 结果
    const result = JSON.parse(output);

    console.log('✅ Analysis complete\n');

    // 显示基本信息
    console.log('📊 Basic Information:');
    console.log(`   File:       ${result.file_path}`);
    console.log(`   Format:     ${result.format}`);
    console.log(`   Size:       ${result.width}x${result.height}`);
    console.log(`   File Size:  ${(result.file_size / 1024).toFixed(2)} KB`);
    console.log(`   Lossless:   ${result.is_lossless ? 'Yes ✓' : 'No'}`);
    console.log(`   Color:      ${result.color_depth}-bit ${result.color_space}`);
    console.log(`   Alpha:      ${result.has_alpha ? 'Yes' : 'No'}`);
    console.log(`   Animated:   ${result.is_animated ? 'Yes' : 'No'}`);

    // 显示质量指标
    if (result.psnr !== null || result.ssim !== null) {
        console.log('\n📈 Quality Metrics:');
        if (result.psnr !== null) {
            console.log(`   PSNR:       ${result.psnr.toFixed(2)} dB`);
        }
        if (result.ssim !== null) {
            console.log(`   SSIM:       ${result.ssim.toFixed(4)}`);
        }
    }

    // 显示升级建议
    if (result.recommendation) {
        const rec = result.recommendation;
        console.log('\n💡 Upgrade Recommendation:');
        console.log(`   From:       ${rec.current_format}`);
        console.log(`   To:         ${rec.recommended_format}`);
        console.log(`   Quality:    ${rec.quality_preservation}`);
        console.log(`   Savings:    ${rec.expected_size_reduction.toFixed(1)}%`);
        console.log(`   Reason:     ${rec.reason}`);
        console.log(`\n   Command:    ${rec.command}`);
    }

    // 返回完整的JSON对象（可用于进一步处理）
    console.log('\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━');
    console.log('💾 Full JSON Response:');
    console.log(JSON.stringify(result, null, 2));

} catch (error) {
    console.error('❌ Error executing imgquality:', error.message);
    process.exit(1);
}

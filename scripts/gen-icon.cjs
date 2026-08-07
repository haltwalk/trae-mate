// 临时脚本:用 @resvg/resvg-js 把 assets/icon.svg 渲染成 1024x1024 PNG,供 tauri icon 生成多尺寸图标
const { Resvg } = require('@resvg/resvg-js')
const fs = require('fs')
const path = require('path')

const svgPath = path.join(__dirname, '..', 'assets', 'icon.svg')
const pngPath = path.join(__dirname, '..', 'assets', 'icon-source.png')

const svg = fs.readFileSync(svgPath)
const resvg = new Resvg(svg, { fitTo: { mode: 'width', value: 1024 } })
const png = resvg.render().asPng()
fs.writeFileSync(pngPath, png)
console.log('PNG generated:', pngPath, 'bytes:', png.length)

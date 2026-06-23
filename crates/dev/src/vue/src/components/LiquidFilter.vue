<template>
  <svg
    style="
      position: absolute;
      width: 0;
      height: 0;
      overflow: hidden;
      pointer-events: none;
    "
    aria-hidden="true"
  >
    <defs>
      <!-- 🍎 Apple macOS Liquid Glass Filter (Authentic Implementation) -->
      <filter
        id="real-liquid"
        x="0%"
        y="0%"
        width="100%"
        height="100%"
        filterUnits="objectBoundingBox"
      >
        <!-- 1. 生成分形噪声 -->
        <feTurbulence
          type="fractalNoise"
          baseFrequency="0.01 0.01"
          numOctaves="1"
          seed="5"
          result="turbulence"
        />

        <!-- 2. Gamma 校正 - 创造 SDF-like 外观 -->
        <feComponentTransfer in="turbulence" result="mapped">
          <feFuncR type="gamma" amplitude="1" exponent="10" offset="0.5" />
          <feFuncG type="gamma" amplitude="0" exponent="1" offset="0" />
          <feFuncB type="gamma" amplitude="0" exponent="1" offset="0.5" />
        </feComponentTransfer>

        <!-- 3. 模糊软化 -->
        <feGaussianBlur in="turbulence" stdDeviation="3" result="softMap" />

        <!-- 4. 镜面高光 -->
        <feSpecularLighting
          in="softMap"
          surfaceScale="5"
          specularConstant="1"
          specularExponent="100"
          lighting-color="white"
          result="specLight"
        >
          <fePointLight x="-200" y="-200" z="300" />
        </feSpecularLighting>

        <!-- 5. 合成 -->
        <feComposite
          in="specLight"
          operator="arithmetic"
          k1="0"
          k2="1"
          k3="1"
          k4="0"
          result="litImage"
        />

        <!-- 6. 位移映射 - 核心折射效果 -->
        <feDisplacementMap
          in="SourceGraphic"
          in2="softMap"
          scale="150"
          xChannelSelector="R"
          yChannelSelector="G"
        />
      </filter>
    </defs>
  </svg>
</template>

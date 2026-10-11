import { mount } from 'svelte'
import '../styles/kit.css'
import Demo from './Demo.svelte'

const target = document.getElementById('app')
if (!target) throw new Error('#app missing')
mount(Demo, { target })

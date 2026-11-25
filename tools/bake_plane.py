#!/usr/bin/env python3
"""Bakes the Sketchfab glTF "Southwest Airlines Boeing 737" (CC-BY-4.0, A Random Modeler) into the compact
binary the app embeds (assets/plane.bin).  Usage: bake_plane.py scene.gltf scene.bin plane.bin
Output body axes, metres: x = right, y = forward (nose), z = up.  Layout (little endian):
u32 vertex count, u32 index count, vertices (pos f32x3, normal i8x4 snorm, rgba u8x4), indices u32."""
import json, numpy as np
import sys
gltf, binf, outf = sys.argv[1:4]
g=json.load(open(gltf)); B=open(binf,'rb').read()
CT={5120:np.int8,5121:np.uint8,5122:np.int16,5123:np.uint16,5125:np.uint32,5126:np.float32}
NC={'SCALAR':1,'VEC2':2,'VEC3':3,'VEC4':4,'MAT4':16}
def acc(i):
    a=g['accessors'][i]; bv=g['bufferViews'][a['bufferView']]
    dt=np.dtype(CT[a['componentType']]); n=NC[a['type']]
    off=bv.get('byteOffset',0)+a.get('byteOffset',0)
    stride=bv.get('byteStride') or dt.itemsize*n
    if stride==dt.itemsize*n:
        arr=np.frombuffer(B,dtype=dt,count=a['count']*n,offset=off).reshape(a['count'],n)
    else:
        arr=np.stack([np.frombuffer(B,dtype=dt,count=n,offset=off+k*stride) for k in range(a['count'])])
    return arr
def local(n):
    if 'matrix' in n: return np.array(n['matrix'],dtype=np.float64).reshape(4,4).T
    t=np.array(n.get('translation',[0,0,0]));r=n.get('rotation',[0,0,0,1]);s=np.array(n.get('scale',[1,1,1]))
    x,y,z,w=r
    R=np.array([[1-2*(y*y+z*z),2*(x*y-z*w),2*(x*z+y*w)],[2*(x*y+z*w),1-2*(x*x+z*z),2*(y*z-x*w)],[2*(x*z-y*w),2*(y*z+x*w),1-2*(x*x+y*y)]])
    M=np.eye(4);M[:3,:3]=R*s;M[:3,3]=t;return M
P=[];N=[];C=[];I=[];base=0
def mat_color(mi):
    if mi is None: return (1,1,1)
    c=g['materials'][mi].get('pbrMetallicRoughness',{}).get('baseColorFactor')
    return tuple(c[:3]) if c else (1,1,1)
def walk(ni,M):
    global base
    n=g['nodes'][ni]; W=M@local(n)
    if 'mesh' in n:
        for p in g['meshes'][n['mesh']]['primitives']:
            if p.get('mode',4)!=4: continue
            pos=acc(p['attributes']['POSITION']).astype(np.float64)
            wp=(np.c_[pos,np.ones(len(pos))]@W.T)[:,:3]
            if 'NORMAL' in p['attributes']:
                nr=acc(p['attributes']['NORMAL']).astype(np.float64)
                Nm=np.linalg.inv(W[:3,:3]).T
                wn=nr@Nm.T; wn/=np.maximum(np.linalg.norm(wn,axis=1,keepdims=True),1e-12)
            else: wn=np.zeros_like(wp)
            idx=acc(p['indices']).reshape(-1).astype(np.int64)
            P.append(wp);N.append(wn);C.append(np.tile(mat_color(p.get('material')),(len(wp),1)));I.append(idx+base);base+=len(wp)
    for c in n.get('children',[]): walk(c,W)
for r in g['scenes'][0]['nodes']: walk(r,np.eye(4))
P=np.concatenate(P);N=np.concatenate(N);C=np.concatenate(C);I=np.concatenate(I)

# glTF here is Y-up, length along Z (nose +Z), units are centimetres.
P=P*0.01
cz=(P[:,2].min()+P[:,2].max())/2
body=np.stack([-P[:,0], P[:,2]-cz, P[:,1]-4.0],axis=1)
nb=np.stack([-N[:,0], N[:,2], N[:,1]],axis=1)
import struct
v=np.zeros(len(body),dtype=[('p','<f4',3),('n','i1',4),('c','u1',4)])
v['p']=body
v['n'][:,:3]=np.clip(np.round(nb*127),-127,127)
v['c'][:,:3]=np.clip(np.round(C*255),0,255); v['c'][:,3]=255
with open(outf,'wb') as f:
    f.write(struct.pack('<II',len(v),len(I)))
    f.write(v.tobytes()); f.write(I.astype('<u4').tobytes())
print('verts',len(v),'indices',len(I),'extent',body.min(0),body.max(0))
